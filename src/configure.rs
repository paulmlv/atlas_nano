//! `atlas-nano configure`: detect the clients, ask for the settings, test a call, and save.

use std::fs;
use std::io::{self, BufRead as _, Write as _};

use crate::config::{self, Config, DEFAULT_RETRIES, Labeler as Settings, labeling_model};
use crate::ingest::FOLDERS;
use crate::label;
use crate::labeler::{self, Failure, Labeler};
use crate::model::Client;
use crate::paint::{Paint, Theme, Tone};
use crate::prompt::Prompt;
use crate::taxonomy::Taxonomy;

/// A client's labeling model as `provider/model`, with the model's API name.
fn provider_model(client: Client) -> String {
    let provider = if client == Client::Claude {
        "anthropic"
    } else {
        "openai"
    };
    format!("{provider}/{}", labeling_model(client))
}

/// What a client's name covers, for messages.
fn label(client: Client) -> &'static str {
    match client {
        Client::Claude => "Claude Code",
        Client::Codex => "Codex/ChatGPT",
    }
}

/// Ask a question on the terminal, showing what can be answered and the default in brackets.
///
/// An empty hint is left out. An empty answer takes the default. The end of input cancels, so
/// nothing is saved: a closed or empty input never stands for the defaults.
fn ask(question: &str, hint: &str, default: &str) -> Result<String, String> {
    let paint = Paint::stdout(Theme::default());
    let hint = match hint {
        "" => String::new(),
        _ => format!("{} ", paint.dim(&format!("({hint})"))),
    };
    print!(
        "{} {hint}{} ",
        paint.bold(question),
        paint.tone(Tone::Accent, &format!("[{default}]"))
    );
    io::stdout().flush().map_err(|error| error.to_string())?;
    let mut reply = String::new();
    let read = io::stdin()
        .lock()
        .read_line(&mut reply)
        .map_err(|error| error.to_string())?;
    if read == 0 {
        println!();
        return Err(
            "configuration cancelled: the input ended before an answer; nothing was saved"
                .to_string(),
        );
    }
    let reply = reply.trim();
    Ok(if reply.is_empty() {
        default.to_string()
    } else {
        reply.to_string()
    })
}

/// Ask until the answer is one of `options`; `yes` and `no` count as `y` and `n`.
fn choose(question: &str, options: &[&str], default: &str) -> Result<String, String> {
    loop {
        let reply = match ask(question, &options.join(" / "), default)?
            .to_lowercase()
            .as_str()
        {
            "yes" => "y".to_string(),
            "no" => "n".to_string(),
            other => other.to_string(),
        };
        if options.contains(&reply.as_str()) {
            return Ok(reply);
        }
        let hint = format!("please answer one of: {}", options.join(", "));
        println!(
            "  {}",
            Paint::stdout(Theme::default()).tone(Tone::Warning, &hint)
        );
    }
}

fn yes(question: &str, default: bool) -> Result<bool, String> {
    Ok(choose(question, &["y", "n"], if default { "y" } else { "n" })? == "y")
}

/// The notice when the labeler would read another provider's conversations, if it would.
pub fn provider_notice(coverage: &[Client], labeler: Client) -> Option<String> {
    let other = coverage.iter().find(|client| **client != labeler)?;
    let provider = if labeler == Client::Claude {
        "Anthropic"
    } else {
        "OpenAI"
    };
    Some(format!(
        "{} conversations will be sent to {provider}, because {} labels every session",
        label(*other),
        label(labeler)
    ))
}

/// Run one small labeling call with the required flags; only a rejected call is an error.
fn test_call(paint: Paint, client: Client, model: &str) -> Result<(), String> {
    let folder = config::cache_folder()?.join(format!("configure-{}", std::process::id()));
    fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let taxonomy = Taxonomy::built_in();
    let prompt = Prompt::new(&taxonomy, false);
    let labeler = Labeler {
        client,
        model,
        prompt: &prompt,
        schema: labeler::schema(&taxonomy, false),
        folder: &folder,
    };
    let text = "[user] What is the capital of France?\n\n[assistant] Paris.";
    let result = labeler.call(&prompt.conversation("unknown", "unknown", text));
    let _ = fs::remove_dir_all(&folder);
    match result.answer {
        Ok(answer) => {
            let answer = format!("{} / {}", answer.work, answer.topic);
            println!(
                "  {} {}",
                paint.dim(&format!("test call with {model}:")),
                paint.tone(Tone::Done, &answer)
            );
            Ok(())
        }
        Err(Failure::Invalid(reason)) => {
            let note =
                format!("test call with {model} ran, but its answer was not usable: {reason}");
            println!("  {}", paint.tone(Tone::Warning, &note));
            Ok(())
        }
        Err(Failure::Rejected(reason)) => {
            Err(format!("the test call with {model} was rejected: {reason}"))
        }
    }
}

pub fn configure() -> Result<(), String> {
    let home = config::home()?;
    let mut paint = Paint::stdout(Theme::default());
    let mut installed = Vec::new();
    for client in [Client::Claude, Client::Codex] {
        let name = format!("{} ({})", client.name(), label(client));
        match labeler::no_save_support(client) {
            Err(_) => println!(
                "{} {}",
                paint.bold(&format!("{name}:")),
                paint.dim("not installed, skipped")
            ),
            Ok(false) => println!(
                "{} {}",
                paint.bold(&format!("{name}:")),
                paint.dim(
                    "installed, but this version cannot run a call without saving it; \
                     skipped until it is updated"
                )
            ),
            Ok(true) => {
                println!(
                    "{} {}",
                    paint.bold(&format!("{name}:")),
                    paint.tone(Tone::Done, "installed")
                );
                installed.push(client);
            }
        }
    }
    if installed.is_empty() {
        return Err(
            "neither Claude Code nor Codex is installed in a version that can label".to_string(),
        );
    }
    let recorded: Vec<Client> = [Client::Claude, Client::Codex]
        .into_iter()
        .filter(|client| {
            FOLDERS
                .iter()
                .any(|(owner, folder)| owner == client && home.join(folder).is_dir())
        })
        .collect();
    // A question with one possible answer is not asked.
    let covered = match recorded[..] {
        [] => {
            return Err("no Claude Code or Codex sessions were found on this computer".to_string());
        }
        [only] => {
            println!(
                "reading {} sessions, the only ones on this computer",
                label(only)
            );
            vec![only]
        }
        _ => match choose(
            "Which sessions to read?",
            &["claude", "codex", "both"],
            "both",
        )?
        .as_str()
        {
            "both" => recorded.clone(),
            "claude" => vec![Client::Claude],
            _ => vec![Client::Codex],
        },
    };
    // Said once, before the test calls, which an exported API key pays for like any other.
    println!("{}", label::billing_note(paint, &installed));
    // Each client labels with one model for now: Claude Code when it is installed, otherwise Codex.
    let offered: Vec<String> = installed
        .iter()
        .map(|&client| provider_model(client))
        .collect();
    let client = match installed[..] {
        [only] => {
            println!("{} labels, through the only client installed", offered[0]);
            only
        }
        _ => {
            let options: Vec<&str> = offered.iter().map(String::as_str).collect();
            let chosen = choose("Which model labels?", &options, &offered[0])?;
            installed[offered
                .iter()
                .position(|model| *model == chosen)
                .expect("the choice was offered")]
        }
    };
    // Confirmed before any call or save, so the user can stop here and choose again.
    if let Some(notice) = provider_notice(&covered, client) {
        println!("{}", paint.tone(Tone::Warning, &notice));
        if !yes("Continue?", true)? {
            println!(
                "{}",
                paint.dim(
                    "nothing saved: run configure again to choose another model or read one \
                     client's sessions"
                )
            );
            return Ok(());
        }
    }
    let tools = !yes(
        "Leave tool calls out of what is sent (no-tools mode)?",
        false,
    )?;
    let pull_requests = yes("Map conversations to pull requests (experimental)?", false)?;
    let jobs = loop {
        match ask("How many labeling calls at once?", "", "32")?.parse::<usize>() {
            Ok(jobs) if jobs > 0 => break jobs,
            _ => println!(
                "  {}",
                paint.tone(Tone::Warning, "please answer a whole number of at least 1")
            ),
        }
    };
    // Without colors there is nothing to preview.
    if paint.is_on() {
        for theme in Theme::ALL {
            println!("  {:<8} {}", theme.name(), paint.with(theme).sample());
        }
    }
    let names = Theme::ALL.map(Theme::name);
    let chosen = choose("Which color theme?", &names, Theme::default().name())?;
    let theme = Theme::ALL[names
        .iter()
        .position(|name| *name == chosen)
        .expect("offered")];
    paint = paint.with(theme);

    // Only the labeler's call must work; another client's failure is reported and not used.
    println!(
        "{}",
        paint.dim("testing a labeling call for each installed client")
    );
    for &tested in &installed {
        match test_call(paint, tested, labeling_model(tested)) {
            Err(error) if tested != client => {
                let note = format!("warning: {error}; it does not label, so configuration goes on");
                println!("  {}", paint.tone(Tone::Warning, &note));
            }
            result => result?,
        }
    }
    let config = Config {
        unit: "conversation".to_string(),
        coverage: covered,
        theme,
        labeler: Settings {
            client,
            model: labeling_model(client).to_string(),
            tools,
            pull_requests,
            jobs,
            retries: DEFAULT_RETRIES,
        },
    };
    // Decode what will be saved, so a setting `run` would refuse is refused here.
    let config = Config::decode(&config.encode())?;
    let path = config.save()?;
    println!("{} {}", paint.tone(Tone::Done, "saved"), path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_only_when_another_providers_conversations_are_sent() {
        let both = [Client::Claude, Client::Codex];
        let notice = provider_notice(&both, Client::Claude).unwrap();
        assert!(notice.contains("Codex/ChatGPT conversations will be sent to Anthropic"));
        let notice = provider_notice(&both, Client::Codex).unwrap();
        assert!(notice.contains("Claude Code conversations will be sent to OpenAI"));
        assert_eq!(provider_notice(&[Client::Claude], Client::Claude), None);
        assert_eq!(provider_model(Client::Claude), "anthropic/claude-haiku-5-5");
        assert_eq!(provider_model(Client::Codex), "openai/gpt-6-luna");
    }
}
