//! Label the sessions of a run: the cache, the estimate, the parallel calls, and the chunk vote.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal as _, Write as _};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::{self, Config};
use crate::labeler::{self, Answer, Call, Failure, Labeler};
use crate::model::{Client, Kind, Label, PrRole, PullRequest, Session, Usage};
use crate::paint::{Paint, Tone};
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

fn stored(entry: &Value) -> Option<Label> {
    let prs = entry["prs"].as_array()?.iter().filter_map(|pr| {
        Some(PullRequest {
            url: pr["url"].as_str()?.to_string(),
            role: PrRole::from_name(pr["role"].as_str()?)?,
        })
    });
    Some(Label {
        work: entry["work"].as_str()?.to_string(),
        topic: entry["topic"].as_str()?.to_string(),
        prs: prs.collect(),
        model: entry["model"].as_str()?.to_string(),
        from_cache: true,
    })
}

fn store(file: &Mutex<Option<fs::File>>, key: &str, label: &Label) {
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
    if let Some(file) = file
        .lock()
        .expect("the cache lock is not poisoned")
        .as_mut()
    {
        let _ = file.write_all(line.as_bytes());
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

/// Who pays for the calls. Claude Code bills an exported `ANTHROPIC_API_KEY` before a subscription.
fn billing(client: Client, api_key: bool) -> &'static str {
    match client {
        Client::Claude if api_key => {
            "ANTHROPIC_API_KEY is set, so Claude Code bills these calls to that API key: the dollar \
             figure is a charge."
        }
        Client::Claude => {
            "The calls go through your signed-in Claude Code client: on a subscription they use \
             your plan's limits, and the dollar figure is what the API would cost, not a charge."
        }
        Client::Codex => {
            "The calls go through your signed-in Codex client: on a subscription they use your \
             plan's limits, and the dollar figure is what the API would cost, not a charge."
        }
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

/// What labeling returns: each labeled session's label by key, and the run's summary.
pub struct Outcome {
    pub labels: HashMap<String, Label>,
    pub summary: Labeling,
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
    let client = config::client(&settings.client)?;
    let prompt = Prompt::new(taxonomy, settings.pull_requests);
    let schema = labeler::schema(taxonomy, settings.pull_requests);
    // Claude Code caps its window at 200K when this is set to anything but 0 or false.
    let capped = std::env::var("CLAUDE_CODE_DISABLE_1M_CONTEXT")
        .is_ok_and(|value| !["", "0", "false"].contains(&value.trim().to_lowercase().as_str()));
    let budget = budget(client, &settings.model, &prompt.instructions, capped);
    let version = taxonomy.version();
    let schema_text = schema.to_string();

    // A run with nothing to call costs nothing.
    let mut summary = Labeling {
        model: settings.model.clone(),
        cost: Some(Decimal::ZERO),
        ..Labeling::default()
    };
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
        let url = match session.client {
            Client::Codex => session.repository.clone(),
            Client::Claude if settings.pull_requests => session
                .cwd
                .as_deref()
                .map(|cwd| repository(cwd, &mut remotes))
                .filter(|url| !url.is_empty()),
            Client::Claude => None,
        };
        let url = url.map_or("unknown".to_string(), |url| without_credentials(&url));
        let branch = session.branch.as_deref().unwrap_or("unknown");
        let prompts: Vec<String> = chunks
            .iter()
            .map(|chunk| prompt.conversation(&url, branch, chunk))
            .collect();
        // Everything sent besides the conversation is in the key, so changing it relabels.
        let mut parts = vec![
            version.as_str(),
            settings.client.as_str(),
            settings.model.as_str(),
            config.unit.as_str(),
            prompt.instructions.as_str(),
            schema_text.as_str(),
            prompt.codex_system.as_str(),
        ];
        parts.push(if settings.pull_requests {
            "prs"
        } else {
            "no-prs"
        });
        parts.extend(prompts.iter().map(String::as_str));
        plans.push(Plan {
            session,
            key: digest(&parts),
            prompts,
        });
    }

    let cache_folder = config::cache_folder()?;
    fs::create_dir_all(&cache_folder)
        .map_err(|error| format!("{}: {error}", cache_folder.display()))?;
    let cache_path = cache_folder.join("labels.jsonl");
    let cached = if no_cache {
        HashMap::new()
    } else {
        read_cache(&cache_path)
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

    // The estimate: what will be sent, before anything is.
    let calls: usize = pending.iter().map(|plan| plan.prompts.len()).sum();
    let characters: usize = pending
        .iter()
        .flat_map(|plan| &plan.prompts)
        .map(String::len)
        .sum();
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
    let tokens: u64 = pending
        .iter()
        .flat_map(|plan| &plan.prompts)
        .map(|prompt| per_call(prompt.len()))
        .sum();
    let estimate: Option<Decimal> = pending
        .iter()
        .flat_map(|plan| &plan.prompts)
        .map(|prompt| {
            price(
                prices,
                client,
                &settings.model,
                per_call(prompt.len()),
                0,
                200,
            )
        })
        .sum();
    let paint = Paint::stderr(config.theme);
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
        paint.tone(
            Tone::Money,
            &format!("about ${} API-equivalent", cost.round_dp(2)),
        )
    });
    eprintln!("  {characters} characters to send, roughly {tokens} tokens, {cost}");
    if !pending.is_empty() {
        let api_key = std::env::var_os("ANTHROPIC_API_KEY").is_some_and(|key| !key.is_empty());
        eprintln!("  {}", paint.dim(billing(client, api_key)));
    }
    if confirm && !pending.is_empty() {
        eprint!("{} {} ", paint.bold("Proceed?"), paint.dim("(y / n) [n]"));
        io::stderr().flush().ok();
        let mut reply = String::new();
        io::stdin()
            .read_line(&mut reply)
            .map_err(|error| error.to_string())?;
        if !matches!(reply.trim().to_lowercase().as_str(), "y" | "yes") {
            return Err("stopped before labeling".to_string());
        }
    }

    if !pending.is_empty() {
        let folder = cache_folder.join(format!("calls-{}", std::process::id()));
        fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        let labeler = Labeler {
            client,
            model: &settings.model,
            prompt: &prompt,
            schema,
            folder: &folder,
        };
        let tasks: Vec<(usize, &str)> = pending
            .iter()
            .enumerate()
            .flat_map(|(n, plan)| plan.prompts.iter().map(move |prompt| (n, prompt.as_str())))
            .collect();
        let results: Vec<Mutex<Option<Call>>> = tasks.iter().map(|_| Mutex::new(None)).collect();
        let cache_file = Mutex::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&cache_path)
                .ok(),
        );
        // A line cut short by an interrupted run is ended, so the next record starts its own line.
        let cut = fs::read(&cache_path).is_ok_and(|data| data.last().is_some_and(|b| *b != b'\n'));
        if cut
            && let Some(file) = cache_file
                .lock()
                .expect("the cache lock is not poisoned")
                .as_mut()
        {
            let _ = file.write_all(b"\n");
        }
        let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let progress = io::stderr().is_terminal();
        let started = Instant::now();
        if progress {
            eprint!("{}", bar(paint, 0, tasks.len(), started.elapsed()));
        }
        std::thread::scope(|scope| {
            for _ in 0..settings.jobs.min(tasks.len()) {
                scope.spawn(|| {
                    loop {
                        let n = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(plan, prompt)) = tasks.get(n) else {
                            break;
                        };
                        let result = labeler.call(prompt);
                        // A session with one call is stored at once, so an interrupted run keeps it.
                        if let Ok(answer) = &result.answer
                            && pending[plan].prompts.len() == 1
                        {
                            let label = combine(std::slice::from_ref(answer), &settings.model);
                            store(&cache_file, &pending[plan].key, &label);
                        }
                        *results[n].lock().expect("the result lock is not poisoned") = Some(result);
                        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if progress {
                            let line = bar(paint, finished, tasks.len(), started.elapsed());
                            eprint!("\r\x1b[2K{line}");
                        }
                    }
                });
            }
        });
        if progress {
            eprintln!();
        }
        let _ = fs::remove_dir_all(&folder);

        let mut answers: Vec<Vec<Answer>> = pending.iter().map(|_| Vec::new()).collect();
        let mut failures: Vec<Option<Failure>> = pending.iter().map(|_| None).collect();
        // Every call counts, failed or not. Claude Code reports each call's cost; Codex reports
        // tokens, priced call by call so one call's size sets its own rate.
        let mut cost = Some(Decimal::ZERO);
        for ((plan, _), result) in tasks.iter().zip(results) {
            let Some(Call { answer, spent }) = result
                .into_inner()
                .expect("the result lock is not poisoned")
            else {
                continue;
            };
            summary.input_tokens += spent.input_tokens;
            summary.output_tokens += spent.output_tokens;
            let priced = match client {
                Client::Claude => Some(spent.cost.unwrap_or_default()),
                Client::Codex => price(
                    prices,
                    client,
                    &settings.model,
                    spent.input_tokens,
                    spent.cached_tokens,
                    spent.output_tokens,
                ),
            };
            cost = cost.zip(priced).map(|(sum, call)| sum + call);
            match answer {
                Ok(answer) => answers[*plan].push(answer),
                Err(failure) => failures[*plan] = failures[*plan].take().or(Some(failure)),
            }
        }
        summary.cost = cost;
        for ((plan, answers), failure) in pending.iter().zip(answers).zip(failures) {
            if let Some(failure) = failure {
                eprintln!(
                    "{} {} is unlabeled: {}",
                    paint.tone(Tone::Warning, "warning:"),
                    plan.session.key(),
                    failure.message()
                );
                summary.unlabeled += 1;
                continue;
            }
            let label = combine(&answers, &settings.model);
            if plan.prompts.len() > 1 {
                store(&cache_file, &plan.key, &label);
            }
            summary.labeled += 1;
            labels.insert(plan.session.key(), label);
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
    fn says_when_an_api_key_pays_for_claude_calls() {
        assert!(billing(Client::Claude, true).contains("ANTHROPIC_API_KEY"));
        assert!(billing(Client::Claude, false).contains("not a charge"));
        // Codex does not read ANTHROPIC_API_KEY.
        assert!(billing(Client::Codex, true).contains("not a charge"));
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
        fs::remove_file(&path).unwrap();
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
        let file = Mutex::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok(),
        );
        let label = combine(
            &[answer("Debugging", "Security", Some(("u1", "Reviewed")))],
            "model",
        );
        store(&file, "key-1", &label);
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
        let key = digest(&["v1", "claude", "haiku", "conversation", "no-prs", "text"]);
        assert_ne!(
            key,
            digest(&["v2", "claude", "haiku", "conversation", "no-prs", "text"])
        );
        assert_ne!(
            key,
            digest(&[
                "v1",
                "claude",
                "haiku",
                "conversation",
                "no-prs",
                "text more"
            ])
        );
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
