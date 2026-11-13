//! Label the sessions of a run: the cache, the estimate, the parallel calls, and the chunk vote.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal as _, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use rayon::prelude::*;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::{self, Config};
use crate::labeler::{self, Answer, Call, Failure, Labeler, Spent};
use crate::model::{Client, Kind, Label, PrRole, PullRequest, Session, Usage};
use crate::paint::{self, Paint, Tone};
use crate::prices::PriceTable;
use crate::prompt::Prompt;
use crate::taxonomy::Taxonomy;
use crate::text;

/// Characters per token when sizing chunks: a lower bound, so a chunk never overflows.
const CHUNK_CHARS_PER_TOKEN: usize = 2;

/// The measured characters per token of each client's labeler, for the estimate.
fn chars_per_token(client: Client) -> f64 {
    match client {
        Client::Claude => 2.3,
        Client::Codex => 3.6,
    }
}

/// The tokens each client adds to every call with the required flags.
fn fixed_tokens(client: Client) -> u64 {
    match client {
        Client::Claude => 1_425,
        Client::Codex => 5_430,
    }
}

/// The context window assumed for a labeling model, in tokens.
///
/// Codex lists 272K for its GPT-6 models; Claude Haiku 5.5 has a 1M window, unless `capped`: Claude
/// Code's 1M context is turned off. Any other model gets a conservative 200K.
fn window(client: Client, model: &str, capped: bool) -> usize {
    match client {
        Client::Codex if model.starts_with("gpt-6") => 272_000,
        Client::Claude if model.starts_with("claude-haiku") && !capped => 1_000_000,
        _ => 200_000,
    }
}

/// The bytes of conversation one call may carry: 80% of the window, less the fixed context.
fn budget(client: Client, model: &str, instructions: &str, capped: bool) -> usize {
    let fixed = usize::try_from(fixed_tokens(client)).unwrap_or(usize::MAX);
    let tokens = (window(client, model, capped) * 4 / 5)
        .saturating_sub(fixed)
        .saturating_sub(instructions.len() / CHUNK_CHARS_PER_TOKEN);
    tokens * CHUNK_CHARS_PER_TOKEN
}

/// One session's calls and the key its label is stored under.
struct Plan<'a> {
    session: &'a Session,
    prompts: Vec<String>,
    key: String,
}

/// The `origin` remote of the folder a Claude Code session ran in, or of its nearest parent.
fn repository(cwd: &str, known: &mut HashMap<String, String>) -> String {
    if let Some(found) = known.get(cwd) {
        return found.clone();
    }
    let folder = Path::new(cwd).ancestors().find(|folder| folder.is_dir());
    let url = folder
        .and_then(|folder| {
            let mut git = Command::new("git");
            git.arg("-C")
                .arg(folder)
                .args(["remote", "get-url", "origin"]);
            Some(
                String::from_utf8_lossy(&git.output().ok()?.stdout)
                    .trim()
                    .to_string(),
            )
        })
        .unwrap_or_default();
    known.insert(cwd.to_string(), url.clone());
    url
}

/// A repository URL without the user name and password a `scheme://user:secret@host/` URL may
/// carry, so no credential is sent to the labeling model.
fn without_credentials(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            format!("{scheme}://{host}{path}")
        }
        None => url.to_string(),
    }
}

fn digest(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.len().to_le_bytes());
        digest.update(part.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Read the stored labels by key. A line that does not decode, damaged or cut short, is ignored and
/// the lines after it are still read.
fn read_cache(path: &Path) -> HashMap<String, Value> {
    let Ok(data) = fs::read(path) else {
        return HashMap::new();
    };
    data.split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter_map(|entry| Some((entry["key"].as_str()?.to_string(), entry)))
        .collect()
}

/// Whether a file's last byte is not a line break, read without reading the whole file.
fn ends_mid_line(path: &Path) -> bool {
    let mut last = [0];
    fs::File::open(path).is_ok_and(|mut file| {
        file.seek(SeekFrom::End(-1)).is_ok() && file.read_exact(&mut last).is_ok()
    }) && last[0] != b'\n'
}

fn stored(entry: &Value) -> Option<Label> {
    // An unreadable pull request makes the whole entry a miss, so the session is labeled again.
    let prs = entry["prs"].as_array()?.iter().map(|pr| {
        Some(PullRequest {
            url: pr["url"].as_str()?.to_string(),
            role: PrRole::from_name(pr["role"].as_str()?)?,
        })
    });
    Some(Label {
        work: entry["work"].as_str()?.to_string(),
        topic: entry["topic"].as_str()?.to_string(),
        prs: prs.collect::<Option<_>>()?,
        model: entry["model"].as_str()?.to_string(),
        from_cache: true,
    })
}

/// The cache file labels are appended to, until an error: then nothing more is stored, and the
/// first error is kept so the run can warn once.
struct Store {
    file: Option<fs::File>,
    error: Option<io::Error>,
}

impl Store {
    /// Open the cache file for appending, creating it if needed.
    fn open(path: &Path) -> Store {
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut file) => {
                // A line cut short by an interrupted run is ended, so the next record starts its
                // own line.
                if ends_mid_line(path) {
                    let _ = file.write_all(b"\n");
                }
                Store {
                    file: Some(file),
                    error: None,
                }
            }
            Err(error) => Store {
                file: None,
                error: Some(error),
            },
        }
    }
}

fn store(cache: &Mutex<Store>, key: &str, label: &Label) {
    let prs: Vec<Value> = label
        .prs
        .iter()
        .map(|pr| json!({"url": pr.url, "role": pr.role.name()}))
        .collect();
    let entry = json!({
        "key": key, "work": label.work, "topic": label.topic, "prs": prs, "model": label.model,
        "labeled": Utc::now().to_rfc3339(),
    });
    // One write of the whole line to a file opened for appending, so a record written by another
    // run at the same time lands before or after this one, not inside it.
    let line = format!("{entry}\n");
    let mut cache = cache.lock().expect("the cache lock is not poisoned");
    if let Some(file) = cache.file.as_mut()
        && let Err(error) = file.write_all(line.as_bytes())
    {
        cache.file = None;
        cache.error = Some(error);
    }
}

/// Pick the most common value; on a tie, the one that appears first.
fn vote<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for value in values {
        match counts.iter_mut().find(|(seen, _)| *seen == value) {
            Some((_, count)) => *count += 1,
            None => counts.push((value, 1)),
        }
    }
    let most = counts.iter().map(|(_, count)| *count).max().unwrap_or(0);
    counts
        .iter()
        .find(|(_, count)| *count == most)
        .map_or(String::new(), |(value, _)| (*value).to_string())
}

/// Combine a session's chunk answers: a vote per axis, and each pull request once.
fn combine(answers: &[Answer], model: &str) -> Label {
    let mut prs: Vec<PullRequest> = Vec::new();
    for pr in answers.iter().flat_map(|answer| &answer.prs) {
        if !prs.iter().any(|seen| seen.url == pr.url) {
            prs.push(pr.clone());
        }
    }
    Label {
        work: vote(answers.iter().map(|answer| answer.work.as_str())),
        topic: vote(answers.iter().map(|answer| answer.topic.as_str())),
        prs,
        model: model.to_string(),
        from_cache: false,
    }
}

/// The API-equivalent cost of calls that used these tokens, when the model has a price.
fn price(
    prices: &PriceTable,
    client: Client,
    model: &str,
    input: u64,
    cached: u64,
    output: u64,
) -> Option<Decimal> {
    // Claude Code writes each prompt to the one-hour cache; Codex bills plain input.
    let fresh = input.saturating_sub(cached);
    let (plain, written) = if client == Client::Claude {
        (0, fresh)
    } else {
        (fresh, 0)
    };
    let usage = Usage {
        id: String::new(),
        owner: None,
        at: Utc::now(),
        model: model.to_string(),
        fast: false,
        input: plain,
        cached,
        cache_write: 0,
        cache_write_1h: written,
        output,
    };
    prices.cost(client, &usage)
}

/// Show a short time as `45s` or `3m05s`.
fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}

/// Whether `ANTHROPIC_API_KEY` is exported, which Claude Code bills before a subscription.
fn api_key() -> bool {
    std::env::var_os("ANTHROPIC_API_KEY").is_some_and(|key| !key.is_empty())
}

/// The billing note for calls through `clients`: a warning when they are a real charge, else a
/// quiet aside.
pub fn billing_note(paint: Paint, clients: &[Client]) -> String {
    let charged = api_key() && clients.contains(&Client::Claude);
    let note = billing(charged);
    if charged {
        format!("{} {note}", paint.tone(Tone::Warning, "warning:"))
    } else {
        paint.dim(note)
    }
}

/// `value` to one significant figure: the estimate is only good to its order of magnitude.
fn rough(value: Decimal) -> Decimal {
    value
        .round_sf_with_strategy(1, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
        .unwrap_or(value)
        .normalize()
}

/// A count to one significant figure, shortened: 40581319 is `40M`.
fn rough_count(count: u64) -> String {
    let value = rough(Decimal::from(count));
    let (unit, suffix) = if value >= Decimal::from(1_000_000) {
        (Decimal::from(1_000_000), "M")
    } else if value >= Decimal::from(1_000) {
        (Decimal::from(1_000), "k")
    } else {
        (Decimal::ONE, "")
    };
    format!("{}{suffix}", (value / unit).normalize())
}

/// Who pays for the calls. Claude Code bills an exported `ANTHROPIC_API_KEY` before a subscription.
fn billing(charged: bool) -> &'static str {
    if charged {
        "ANTHROPIC_API_KEY is set, so Claude Code bills these calls to that API key: the dollar \
         figure is a charge."
    } else {
        "We label conversations with your signed-in account. On a subscription this counts \
         toward your plan's limits; on an API account it is a charge."
    }
}

/// One line of progress: a bar, the calls done, the share, and the time spent and left.
fn bar(paint: Paint, done: usize, total: usize, elapsed: Duration) -> String {
    const WIDTH: usize = 30;
    let filled = (done * WIDTH).checked_div(total).unwrap_or(WIDTH);
    let percent = (done * 100).checked_div(total).unwrap_or(100);
    let left = match done {
        0 => String::new(),
        _ if done >= total => String::new(),
        _ => {
            let remaining = elapsed.mul_f64((total - done) as f64 / done as f64);
            format!(" · about {} left", clock(remaining))
        }
    };
    let counts = format!("{done}/{total} calls");
    let times = format!(" · {percent}% · {}{left}", clock(elapsed));
    format!(
        "  {}{} {}{}",
        paint.tone(Tone::Accent, &"█".repeat(filled)),
        paint.dim(&"░".repeat(WIDTH - filled)),
        paint.bold(&counts),
        paint.dim(&times)
    )
}

/// What a labeling run did, for the report's last section.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Labeling {
    pub model: String,
    pub labeled: u64,
    pub from_cache: u64,
    pub unlabeled: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// The API-equivalent cost of the calls, when it can be priced.
    pub cost: Option<Decimal>,
}

impl Labeling {
    /// Count one call's tokens and cost. Claude Code reports each call's cost; Codex reports
    /// tokens, priced call by call so one call's size sets its own rate.
    fn add(&mut self, spent: &Spent, client: Client, prices: &PriceTable) {
        self.input_tokens += spent.input_tokens;
        self.output_tokens += spent.output_tokens;
        let priced = match client {
            Client::Claude => Some(spent.cost.unwrap_or_default()),
            Client::Codex => price(
                prices,
                client,
                &self.model,
                spent.input_tokens,
                spent.cached_tokens,
                spent.output_tokens,
            ),
        };
        self.cost = self.cost.zip(priced).map(|(sum, call)| sum + call);
    }
}

/// What labeling returns: each labeled session's label by key, and the run's summary.
pub struct Outcome {
    pub labels: HashMap<String, Label>,
    pub summary: Labeling,
}

/// The repository a session's prompt names, or `unknown`. Codex records it; for Claude Code it is
/// the `origin` remote of the session's folder, looked up only for the pull request mapping.
fn remote(session: &Session, pull_requests: bool, known: &mut HashMap<String, String>) -> String {
    let url = match session.client {
        Client::Codex => session.repository.clone(),
        Client::Claude if pull_requests => session
            .cwd
            .as_deref()
            .map(|cwd| repository(cwd, known))
            .filter(|url| !url.is_empty()),
        Client::Claude => None,
    };
    url.map_or("unknown".to_string(), |url| without_credentials(&url))
}

/// Plan each session's calls: render and chunk it, and build its prompts and its cache key.
/// Safety reviews are left out, and a session that renders to no text counts as unlabeled.
fn plan<'a>(
    sessions: &[&'a Session],
    config: &Config,
    prompt: &Prompt,
    schema: &str,
    summary: &mut Labeling,
) -> Vec<Plan<'a>> {
    let settings = &config.labeler;
    // Claude Code caps its window at 200K when this is set to anything but 0 or false.
    let capped = std::env::var("CLAUDE_CODE_DISABLE_1M_CONTEXT")
        .is_ok_and(|value| !["", "0", "false"].contains(&value.trim().to_lowercase().as_str()));
    let budget = budget(
        settings.client,
        &settings.model,
        &prompt.instructions,
        capped,
    );
    let mut remotes = HashMap::new();
    let mut plans = Vec::new();
    for &session in sessions
        .iter()
        .filter(|session| session.kind != Kind::Review)
    {
        let chunks = text::chunks(&text::render(&session.items, settings.tools), budget);
        if chunks.is_empty() {
            summary.unlabeled += 1;
            continue;
        }
        let url = remote(session, settings.pull_requests, &mut remotes);
        let branch = session.branch.as_deref().unwrap_or("unknown");
        let prompts: Vec<String> = chunks
            .iter()
            .map(|chunk| prompt.conversation(&url, branch, chunk))
            .collect();
        // Everything sent to the labeler, and how, so a changed instruction, label list or effort
        // relabels; and this build's version, so a new release that reads answers differently does
        // too.
        let mut parts = vec![
            env!("CARGO_PKG_VERSION"),
            prompt.instructions.as_str(),
            schema,
            prompt.codex_system.as_str(),
            settings.client.name(),
            settings.model.as_str(),
            labeler::EFFORT,
            config.unit.as_str(),
        ];
        parts.extend(prompts.iter().map(String::as_str));
        plans.push(Plan {
            session,
            key: digest(&parts),
            prompts,
        });
    }
    plans
}

/// The file the labels are stored in, its folder created if needed.
fn cache_path() -> Result<PathBuf, String> {
    let folder = config::cache_folder()?;
    fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    Ok(folder.join("labels.jsonl"))
}

/// Split the plans into the labels already stored, by session key, and the plans still to call.
fn lookup<'a>(
    plans: Vec<Plan<'a>>,
    cache_path: &Path,
    no_cache: bool,
    summary: &mut Labeling,
) -> (HashMap<String, Label>, Vec<Plan<'a>>) {
    let cached = if no_cache {
        HashMap::new()
    } else {
        read_cache(cache_path)
    };
    let mut labels = HashMap::new();
    let mut pending = Vec::new();
    for plan in plans {
        match cached.get(&plan.key).and_then(stored) {
            Some(label) => {
                summary.from_cache += 1;
                labels.insert(plan.session.key(), label);
            }
            None => pending.push(plan),
        }
    }
    (labels, pending)
}

/// Print the estimate: what will be sent, before anything is.
fn estimate(
    pending: &[Plan<'_>],
    summary: &Labeling,
    settings: &config::Labeler,
    prompt: &Prompt,
    prices: &PriceTable,
    paint: Paint,
) {
    let client = settings.client;
    let calls: usize = pending.iter().map(|plan| plan.prompts.len()).sum();
    // Each call sends the instructions, the conversation, and the client's own context.
    let per_call = |characters: usize| -> u64 {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let tokens =
            ((characters + prompt.instructions.len()) as f64 / chars_per_token(client)) as u64;
        tokens + fixed_tokens(client)
    };
    let sizes: Vec<u64> = pending
        .iter()
        .flat_map(|plan| &plan.prompts)
        .map(|conversation| per_call(conversation.len()))
        .collect();
    let tokens: u64 = sizes.iter().sum();
    let estimate: Option<Decimal> = sizes
        .iter()
        .map(|&size| price(prices, client, &settings.model, size, 0, 200))
        .sum();
    eprintln!(
        "{} {}",
        paint.tone(
            Tone::Heading,
            &format!(
                "Labeling {} sessions in {calls} calls with {}",
                pending.len(),
                settings.model
            )
        ),
        paint.dim(&format!(
            "({} from cache, {} with no text)",
            summary.from_cache, summary.unlabeled
        ))
    );
    let cost = estimate.map_or("no price for this model".to_string(), |cost| {
        paint.tone(Tone::Money, &format!("${}", rough(cost)))
    });
    eprintln!(
        "  Estimated tokens: {} · API-equivalent: {cost}",
        rough_count(tokens)
    );
    if !pending.is_empty() {
        eprintln!("  {}", billing_note(paint, &[client]));
    }
}

/// Ask whether to proceed. Any reply but yes stops the run.
fn confirmed(paint: Paint) -> Result<(), String> {
    eprint!("{} {} ", paint.bold("Proceed?"), paint.dim("(y / n) [n]"));
    io::stderr().flush().ok();
    let mut reply = String::new();
    io::stdin()
        .read_line(&mut reply)
        .map_err(|error| error.to_string())?;
    if matches!(reply.trim().to_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err("stopped before labeling".to_string())
    }
}

/// One call to make: a prompt, and the plan it is a chunk of.
#[derive(Clone, Copy)]
struct Task<'a> {
    plan: &'a Plan<'a>,
    prompt: &'a str,
}

/// What every pass of calls shares.
struct Passes<'a> {
    labeler: &'a Labeler<'a>,
    pool: rayon::ThreadPool,
    cache: &'a Mutex<Store>,
    paint: Paint,
}

impl Passes<'_> {
    /// Make the calls of `tasks` in parallel, with a progress bar: each task's call, in order.
    fn run(&self, tasks: &[Task<'_>]) -> Vec<Call> {
        let progress = io::stderr().is_terminal();
        let done = AtomicUsize::new(0);
        let started = Instant::now();
        // Each line is redrawn over the last with a carriage return and padded to the widest so far,
        // so no escape code is needed to clear what a longer line left.
        let widest = AtomicUsize::new(0);
        let draw = |done: usize| {
            if !progress {
                return;
            }
            let line = bar(self.paint, done, tasks.len(), started.elapsed());
            let width = paint::width(&line);
            let pad = widest
                .fetch_max(width, Ordering::Relaxed)
                .saturating_sub(width);
            eprint!("\r{line}{}", " ".repeat(pad));
        };
        draw(0);
        // One call per task, so an idle thread can always take the next one.
        let calls = self.pool.install(|| {
            tasks
                .par_iter()
                .with_max_len(1)
                .map(|task| {
                    let call = self.labeler.call(task.prompt);
                    // A session with one call is stored at once, so an interrupted run keeps it.
                    if let Ok(answer) = &call.answer
                        && task.plan.prompts.len() == 1
                    {
                        let label = combine(std::slice::from_ref(answer), self.labeler.model);
                        store(self.cache, &task.plan.key, &label);
                    }
                    draw(done.fetch_add(1, Ordering::Relaxed) + 1);
                    call
                })
                .collect()
        });
        if progress {
            eprintln!();
        }
        calls
    }
}

/// The memory one labeling call is allowed, in bytes. A Claude Code process takes about 250 MB, part
/// of it shared with the other calls; this leaves room for longer conversations and other programs.
const MEMORY_PER_CALL: u64 = 500 * 1024 * 1024;

/// How many labeling calls a computer with `cores` cores and `memory` bytes runs at once without
/// slowing down: three per core, and one per `MEMORY_PER_CALL`, whichever is fewer. With the
/// memory unknown, only the cores count.
fn limit(cores: usize, memory: Option<u64>) -> usize {
    let by_cores = 3 * cores;
    let by_memory = memory.map_or(by_cores, |bytes| {
        usize::try_from(bytes / MEMORY_PER_CALL).unwrap_or(usize::MAX)
    });
    by_cores.min(by_memory).max(1)
}

/// The total memory of this computer in bytes, or `None` if it cannot be read.
fn memory() -> Option<u64> {
    if cfg!(target_os = "macos") {
        let output = Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    } else {
        let info = fs::read_to_string("/proc/meminfo").ok()?;
        let line = info.lines().find(|line| line.starts_with("MemTotal:"))?;
        let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kilobytes * 1024)
    }
}

/// The most labeling calls this computer runs at once without slowing down. Past it, the calls
/// wait for a core or for memory, and a run takes longer, not shorter.
pub fn jobs_limit() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    limit(cores, memory())
}

/// Call the labeler for every prompt of `pending`. A failed call is tried again, up to `retries`
/// more times, each time once every call of the pass before has finished. Every call counts in
/// the summary, failed or not.
///
/// Returns each call's last result, in the order of the plans and of each plan's prompts.
fn call(
    pending: &[Plan<'_>],
    labeler: &Labeler<'_>,
    cache: &Mutex<Store>,
    settings: &config::Labeler,
    prices: &PriceTable,
    paint: Paint,
    summary: &mut Labeling,
) -> Result<Vec<Result<Answer, Failure>>, String> {
    let tasks: Vec<Task> = pending
        .iter()
        .flat_map(|plan| plan.prompts.iter().map(move |prompt| Task { plan, prompt }))
        .collect();
    let limit = jobs_limit();
    if settings.jobs > limit {
        eprintln!(
            "{} jobs = {} is more calls at once than this computer can run without slowing down, \
             so {limit} run at once",
            paint.tone(Tone::Warning, "warning:"),
            settings.jobs
        );
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(settings.jobs.min(limit).min(tasks.len()))
        .build()
        .map_err(|error| error.to_string())?;
    let passes = Passes {
        labeler,
        pool,
        cache,
        paint,
    };
    // One pass over the given tasks: each one's result, its tokens and cost counted.
    let mut pass = |todo: &[Task<'_>]| -> Vec<Result<Answer, Failure>> {
        let calls = passes.run(todo);
        calls
            .into_iter()
            .map(|Call { answer, spent }| {
                summary.add(&spent, labeler.client, prices);
                answer
            })
            .collect()
    };

    let mut results = pass(&tasks);
    for _ in 0..settings.retries {
        let failed: Vec<usize> = (0..tasks.len())
            .filter(|&task| results[task].is_err())
            .collect();
        if failed.is_empty() {
            break;
        }
        eprintln!(
            "{}",
            paint.tone(
                Tone::Heading,
                &format!("Retrying {} failed calls", failed.len())
            )
        );
        let again: Vec<Task> = failed.iter().map(|&task| tasks[task]).collect();
        for (task, result) in failed.into_iter().zip(pass(&again)) {
            results[task] = result;
        }
    }
    Ok(results)
}

/// Turn the results of the calls into each session's label, by session key. `results` holds the
/// calls of each plan of `pending` in turn.
///
/// A session is unlabeled when any of its calls failed. A label combined from several chunks is
/// stored here; a single call's label was stored when the call returned.
fn collect(
    pending: &[Plan<'_>],
    results: Vec<Result<Answer, Failure>>,
    cache: &Mutex<Store>,
    paint: Paint,
    summary: &mut Labeling,
) -> HashMap<String, Label> {
    let mut labels = HashMap::new();
    let mut results = results.into_iter();
    for plan in pending {
        // All of the plan's results are taken first, so the next plan starts at its own.
        let calls: Vec<_> = results.by_ref().take(plan.prompts.len()).collect();
        // The first failure, or every answer.
        match calls.into_iter().collect::<Result<Vec<Answer>, Failure>>() {
            Err(failure) => {
                eprintln!(
                    "{} {} is unlabeled: {}",
                    paint.tone(Tone::Warning, "warning:"),
                    paint.tone(Tone::Plain, &plan.session.key()),
                    paint.tone(Tone::Plain, failure.message())
                );
                summary.unlabeled += 1;
            }
            Ok(answers) => {
                let label = combine(&answers, &summary.model);
                if plan.prompts.len() > 1 {
                    store(cache, &plan.key, &label);
                }
                summary.labeled += 1;
                labels.insert(plan.session.key(), label);
            }
        }
    }
    labels
}

/// Label `sessions`, reusing stored labels unless `no_cache`, and asking first with `confirm`.
pub fn label(
    sessions: &[&Session],
    config: &Config,
    taxonomy: &Taxonomy,
    prices: &PriceTable,
    no_cache: bool,
    confirm: bool,
) -> Result<Outcome, String> {
    let settings = &config.labeler;
    let prompt = Prompt::new(taxonomy, settings.pull_requests);
    let schema = labeler::schema(taxonomy, settings.pull_requests);
    // A run with nothing to call costs nothing.
    let mut summary = Labeling {
        model: settings.model.clone(),
        cost: Some(Decimal::ZERO),
        ..Labeling::default()
    };

    let plans = plan(sessions, config, &prompt, &schema.to_string(), &mut summary);
    let cache_path = cache_path()?;
    let (mut labels, pending) = lookup(plans, &cache_path, no_cache, &mut summary);
    let paint = Paint::stderr(config.theme);
    estimate(&pending, &summary, settings, &prompt, prices, paint);
    if !pending.is_empty() {
        if confirm {
            confirmed(paint)?;
        }
        let folder = config::call_folder()?;
        let labeler = Labeler {
            client: settings.client,
            model: &settings.model,
            prompt: &prompt,
            schema,
            folder: &folder,
        };
        let cache = Mutex::new(Store::open(&cache_path));
        let results = call(
            &pending,
            &labeler,
            &cache,
            settings,
            prices,
            paint,
            &mut summary,
        )?;
        let _ = fs::remove_dir_all(&folder);
        labels.extend(collect(&pending, results, &cache, paint, &mut summary));
        let stored = cache.into_inner().expect("the cache lock is not poisoned");
        if let Some(error) = stored.error {
            // Shown like the other paths, with any control character in it as data.
            eprintln!(
                "{} could not store labels in {}: {error}; the next run labels these sessions \
                 again",
                paint.tone(Tone::Warning, "warning:"),
                paint.tone(Tone::Plain, &cache_path.display().to_string())
            );
        }
    }
    if summary.unlabeled > 0 {
        eprintln!(
            "{} {} sessions are unlabeled",
            paint.tone(Tone::Warning, "warning:"),
            summary.unlabeled
        );
    }
    Ok(Outcome { labels, summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_calls_at_once_follow_cores_and_memory() {
        const GB: u64 = 1024 * 1024 * 1024;
        assert_eq!(limit(10, Some(16 * GB)), 30, "the cores");
        assert_eq!(limit(8, Some(8 * GB)), 16, "the memory");
        assert_eq!(limit(16, Some(64 * GB)), 48);
        assert_eq!(limit(4, None), 12, "memory unknown");
        assert_eq!(limit(1, Some(GB / 4)), 1, "at least one");
    }

    #[test]
    fn rounds_the_estimate_to_one_figure() {
        assert_eq!(rough(Decimal::new(1944, 2)).to_string(), "20");
        assert_eq!(rough(Decimal::new(41_230, 2)).to_string(), "400");
        assert_eq!(rough(Decimal::new(34, 3)).to_string(), "0.03");
        assert_eq!(rough(Decimal::new(15, 1)).to_string(), "2");
        assert_eq!(rough_count(74_347_701), "70M");
        assert_eq!(rough_count(40_581_319), "40M");
        assert_eq!(rough_count(999_600), "1M");
        assert_eq!(rough_count(2_345), "2k");
        assert_eq!(rough_count(87), "90");
        assert_eq!(rough_count(0), "0");
    }

    #[test]
    fn says_when_an_api_key_pays_for_claude_calls() {
        assert!(billing(true).contains("ANTHROPIC_API_KEY"));
        assert!(billing(false).contains("plan's limits"));
    }

    #[test]
    fn sends_no_credentials_from_a_repository_url() {
        let strip = without_credentials;
        assert_eq!(
            strip("https://user:TOKEN@github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            strip("https://github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(strip("ssh://git@host:22/o/r"), "ssh://host:22/o/r");
        assert_eq!(strip("git@github.com:o/r.git"), "git@github.com:o/r.git");
    }

    #[test]
    fn reads_the_cache_past_a_damaged_line() {
        let path = std::env::temp_dir().join(format!("atlas-nano-damaged-{}", std::process::id()));
        let mut data = b"{\"key\": \"a\"}\n".to_vec();
        data.extend_from_slice(b"\xff\xfe not json\n{\"key\": \"b\"}\n{\"key\": \"c");
        fs::write(&path, data).unwrap();
        let cache = read_cache(&path);
        assert!(ends_mid_line(&path));
        fs::write(&path, b"{\"key\": \"a\"}\n").unwrap();
        assert!(!ends_mid_line(&path));
        fs::remove_file(&path).unwrap();
        assert!(!ends_mid_line(&path));
        let mut keys: Vec<&str> = cache.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["a", "b"]);
    }

    fn answer(work: &str, topic: &str, pr: Option<(&str, &str)>) -> Answer {
        Answer {
            work: work.to_string(),
            topic: topic.to_string(),
            prs: pr
                .into_iter()
                .map(|(url, role)| PullRequest {
                    url: url.into(),
                    role: PrRole::from_name(role).unwrap(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_progress_bar_fills_with_the_calls_done() {
        let seconds = Duration::from_secs;
        assert_eq!(
            bar(Paint::OFF, 0, 314, seconds(0)),
            format!("  {} 0/314 calls · 0% · 0s", "░".repeat(30))
        );
        assert_eq!(
            bar(Paint::OFF, 100, 300, seconds(50)),
            format!(
                "  {}{} 100/300 calls · 33% · 50s · about 1m40s left",
                "█".repeat(10),
                "░".repeat(20)
            )
        );
        assert_eq!(
            bar(Paint::OFF, 300, 300, seconds(150)),
            format!("  {} 300/300 calls · 100% · 2m30s", "█".repeat(30))
        );
    }

    #[test]
    fn chunks_vote_per_axis_and_ties_go_to_the_first() {
        let answers = [
            answer("Debugging", "Security", Some(("u1", "Reviewed"))),
            answer("Writing code", "Security", Some(("u1", "Mentioned only"))),
            answer(
                "Writing code",
                "Backend and APIs",
                Some(("u2", "Contributed to")),
            ),
            answer("Debugging", "Frontend and UI", None),
        ];
        let label = combine(&answers, "model");
        assert_eq!(
            (label.work.as_str(), label.topic.as_str()),
            ("Debugging", "Security")
        );
        let prs: Vec<(&str, &str)> = label
            .prs
            .iter()
            .map(|pr| (pr.url.as_str(), pr.role.name()))
            .collect();
        assert_eq!(prs, [("u1", "Reviewed"), ("u2", "Contributed to")]);
        assert!(!label.from_cache);
    }

    #[test]
    fn a_stored_label_round_trips_through_the_cache_file() {
        let folder = std::env::temp_dir().join(format!("atlas-nano-cache-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("labels.jsonl");
        let file = Mutex::new(Store::open(&path));
        let label = combine(
            &[answer("Debugging", "Security", Some(("u1", "Reviewed")))],
            "model",
        );
        store(&file, "key-1", &label);
        assert!(file.lock().unwrap().error.is_none());
        // A write that fails stops the storing and keeps the error for the run's warning.
        let read_only = Mutex::new(Store {
            file: fs::File::open(&path).ok(),
            error: None,
        });
        store(&read_only, "key-2", &label);
        let stopped = read_only.into_inner().unwrap();
        assert!(stopped.file.is_none() && stopped.error.is_some());
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not json\n")
            .unwrap();
        let cache = read_cache(&path);
        assert_eq!(cache.len(), 1);
        assert_eq!(
            stored(&cache["key-1"]),
            Some(Label {
                from_cache: true,
                ..label
            })
        );
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn the_key_changes_with_anything_a_label_depends_on() {
        let key = digest(&["rules", "schema", "claude", "haiku", "conversation", "text"]);
        for changed in [
            [
                "other rules",
                "schema",
                "claude",
                "haiku",
                "conversation",
                "text",
            ],
            [
                "rules",
                "other schema",
                "claude",
                "haiku",
                "conversation",
                "text",
            ],
            [
                "rules",
                "schema",
                "claude",
                "haiku",
                "conversation",
                "text more",
            ],
        ] {
            assert_ne!(key, digest(&changed));
        }
        assert_ne!(digest(&["ab", "c"]), digest(&["a", "bc"]));
    }

    #[test]
    fn the_budget_leaves_a_fifth_of_the_window_and_the_fixed_context() {
        assert_eq!(
            budget(Client::Codex, "gpt-6-luna", "", false),
            (217_600 - 5_430) * 2
        );
        assert_eq!(
            budget(
                Client::Claude,
                "claude-haiku-5-5",
                &"x".repeat(2_000),
                false
            ),
            (800_000 - 1_425 - 1_000) * 2
        );
        assert_eq!(window(Client::Claude, "something-else", false), 200_000);
        // With Claude Code's 1M context off, Haiku gets 200K.
        assert_eq!(window(Client::Claude, "claude-haiku-5-5", true), 200_000);
        assert_eq!(window(Client::Codex, "gpt-6-luna", true), 272_000);
    }
}
