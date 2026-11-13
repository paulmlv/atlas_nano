# Command line

`atlas-nano` is one binary for macOS and Linux with two commands. It reads only local files and
makes no network request of its own. Labeling goes through the installed Claude Code or Codex client
the user is signed in to: on a subscription it counts against the plan's usage limits and is not
billed as API calls, and every dollar figure is what the API would cost, not a charge. The exception
is an exported `ANTHROPIC_API_KEY`: Claude Code bills that key, and its dollar figures are charges.
Both commands say which applies before any call.

Output to a terminal is in color, in the theme chosen in `configure`. Output to a file or a pipe is
plain text, and so is any output when `NO_COLOR` is set to a non-empty value, `CLICOLOR` is `0`, or
`TERM` is `dumb` or unset. A non-empty `CLICOLOR_FORCE` turns color on, even for a pipe. These are
the rules clap follows for its help. Plain output has no escape codes at all.

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
  model, the no-tools mode, the pull request mapping (on by default), how many labeling calls run at
  once (32 by default; asked again until it is a whole number of at least 1), and the color theme.
  It also saves `retries = 1` without asking: how many more times a failed labeling call is tried.
  Edit the file to change it; `0` turns retries off. A file saved without it gets 1.
- **Theme:** `minimal` (the default) uses bold, dim, and the terminal's own colors, so it suits any
  background. `soft` has muted blue headings and sage money; `warm` has amber headings and bars.
  Both use fixed 256-color shades. On a terminal, each theme is previewed before the question.
- **Labeling model:** `anthropic/claude-haiku-5-5` through Claude Code, or `openai/gpt-6-luna`
  through Codex. With one client installed, its model labels and the question is not asked. No other
  model is accepted, in `configure` or in the saved file.
- **Test call:** for each installed client, configuration runs one small labeling call with the
  [required flags](LABELING.md#required-flags). Configuration fails if the chosen labeler's call is
  rejected, as when its client is not signed in; another client's rejected call is a warning. A
  missing or invalid answer does not fail configuration.
- **Provider confirmation:** when the chosen labeler would send one client's conversations to the
  other provider, configuration says so right after the labeler is chosen and asks to continue,
  yes by default, before any call or save. A no stops it with nothing saved. This arises when both
  clients are covered, since one labeler reads everything.

## `atlas-nano run`

Requires a saved configuration; without one it stops and says to run `configure`.

| Flag            | Default   | Meaning                                                            |
| --------------- | --------- | ------------------------------------------------------------------ |
| `--period`      | `7d`      | Time horizon: `1d`, `7d`, `30d`, or `all`                          |
| `--no-cache`    | cache on  | Relabel everything, ignoring stored labels                         |
| `--csv <path>`  | no file   | Also write the per-session [CSV](REPORT.md#csv)                    |
| `--no-label`    | label     | Accounting only, with no model calls                               |
| `--confirm`     | no prompt | Ask before labeling, after the estimate is printed                 |
| `--retries <N>` | saved     | Try a [failed call](LABELING.md#the-call) N more times; `0` is off |

### Period

- A conversation is in the period when it started within it, measured from the exact time of the
  run: `1d` is the last 24 hours, `7d` the last 168, and `30d` the last 720. Run at noon, a
  conversation started seven days ago at 10:00 is out of `7d`.
- A conversation that is in counts with its whole spend, turns, and time.
- A subagent is in the report exactly when the chat that spawned it is.

### Missing usage

A session in the period (a conversation, subagent, or review) that has replies but no usage Atlas
Nano could read counts as $0, so the run prints a warning on standard error with the number of such
sessions for each client. This happens, for example, when a client changes how it records usage or
does not record it. Usage counts as read only when it has both input and output token counts.

### Labeling cost

- After reading all sessions and before labeling, the run prints an estimate on standard error: the
  sessions to label, the calls, and the estimated tokens and API-equivalent cost. Each call's tokens
  count the labeling instructions and the conversation, at 2.3 characters per token for a Claude
  labeler and 3.6 for a Codex labeler, plus the client's fixed context. The cost assumes Claude
  Code writes each prompt to its one-hour cache, as it does. The tokens and cost are shown to one
  significant figure, such as `40M` and `$20`, since the estimate is only good to its order of
  magnitude.
- By default the run then proceeds. With `--confirm` it asks first.
- While the calls run, a progress bar on standard error shows the calls done, the time spent, and an
  estimate of the time left. Each retry of failed calls prints how many it retries and has its own
  bar.
- After labeling, the run prints the tokens actually used and their API-equivalent cost.

### Label cache

Labels are stored in `~/.cache/atlas-nano/labels.jsonl`, or under `$XDG_CACHE_HOME` when that is
set. A stored label is reused only when everything sent to the labeler (the instructions, the label
lists, the answer schema, and the conversation's text) and the labeler, its effort and the unit are unchanged, so a
conversation that grew, or any change to the prompt, labels it again. A
session whose call failed is not stored, so the next run tries it again.
