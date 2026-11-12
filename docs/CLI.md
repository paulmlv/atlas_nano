# Command line

`atlas-nano` is one binary for macOS and Linux with two commands. It reads only local files and
makes no network request of its own. Labeling goes through the installed Claude Code or Codex client
the user is signed in to: on a subscription it counts against the plan's usage limits and is not
billed as API calls, and every dollar figure is what the API would cost, not a charge. Both commands
say so.

## `atlas-nano configure`

Asks its questions on the terminal and saves the answers to `~/.config/atlas-nano/config.toml`, or
under `$XDG_CONFIG_HOME` when that is set. Each question shows the possible answers in parentheses
and the default in brackets; an empty answer takes the default. If the input ends before every
question is answered, configuration stops and saves nothing. A question with one possible answer is
not asked. `codex` covers Codex and ChatGPT desktop sessions, and `claude` Claude Code's.

- **Detection:** Claude Code and Codex are checked separately, along with their session folders
  (`~/.claude/projects`, `~/.codex/sessions`, `~/.codex/archived_sessions`). A client that is not
  installed, or whose version lacks its no-save flag, is skipped. With neither usable, configuration
  fails.
- **Settings:** the unit (conversation), the coverage (Claude Code, Codex, or both), the labeling
  model, the no-tools mode, the experimental pull request mapping, and how many labeling calls run
  at once (32 by default; asked again until it is a whole number of at least 1).
- **Labeling model:** `anthropic/claude-haiku-5-5` through Claude Code, or `openai/gpt-6-luna`
  through Codex. With one client installed, its model labels and the question is not asked. No other
  model is accepted, in `configure` or in the saved file.
- **Test call:** for each installed client, configuration runs one small labeling call with the
  [required flags](LABELING.md#required-flags). Configuration fails if the chosen labeler's call is
  rejected, as when its client is not signed in; another client's rejected call is a warning. A
  missing or invalid answer does not fail configuration.
- **Provider warning:** when the chosen labeler would send one client's conversations to the other
  provider, configuration prints a warning right after the labeler is chosen, before any call or
  save. This arises when both clients are covered, since one labeler reads everything.

## `atlas-nano run`

Requires a saved configuration; without one it stops and says to run `configure`.

| Flag           | Default   | Meaning                                            |
| -------------- | --------- | -------------------------------------------------- |
| `--period`     | `7d`      | Time horizon: `1d`, `7d`, `30d`, or `all`          |
| `--no-cache`   | cache on  | Relabel everything, ignoring stored labels         |
| `--csv <path>` | no file   | Also write the per-session [CSV](REPORT.md#csv)    |
| `--no-label`   | label     | Accounting only, with no model calls               |
| `--confirm`    | no prompt | Ask before labeling, after the estimate is printed |

### Period

- A conversation is in the period when it started within it, measured from the exact time of the
  run: `1d` is the last 24 hours, `7d` the last 168, and `30d` the last 720. Run at noon, a
  conversation started seven days ago at 10:00 is out of `7d`.
- A conversation that is in counts with its whole spend, turns, and time.
- A subagent is in the report exactly when the chat that spawned it is.

### Labeling cost

- After reading all sessions and before labeling, the run prints an estimate on standard error: the
  sessions to label, the calls, the characters to send, and a rough token count and API-equivalent
  cost. Tokens are estimated at 2.3 characters per token for a Claude labeler and 3.6 for a Codex
  labeler, plus each client's fixed context per call. The cost assumes Claude Code writes each
  prompt to its one-hour cache, as it does.
- By default the run then proceeds. With `--confirm` it asks first.
- While the calls run, a progress bar on standard error shows the calls done, the time spent, and an
  estimate of the time left.
- After labeling, the run prints the tokens actually used and their API-equivalent cost.

### Label cache

Labels are stored in `~/.cache/atlas-nano/labels.jsonl`, or under `$XDG_CACHE_HOME` when that is
set. A stored label is reused only when the text sent, the taxonomy, the labeler, the unit, and the
pull request mapping setting are all unchanged, so a conversation that grew is labeled again. A
session whose call failed is not stored, so the next run tries it again.
