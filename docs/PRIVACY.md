# Privacy

Atlas Nano reads local Claude Code and Codex session files. It sends no conversation data to Atlas
servers and has no telemetry of its own. Labeling sends selected conversation text through your
installed Claude Code or Codex client.

## What is sent

- User and assistant text, messages from other agents, and each tool call's name and full input.
  Tool calls are included by default; turn them off in `configure` to omit them.
- Tool results, thinking, and injected context are omitted. An image the user attached becomes an
  `[image]` marker. The [labeling rules](LABELING.md#what-is-sent) list the full selection.
- With pull request mapping on, the known repository URL and branch are included. The URL's user
  name and password are removed.

Kept text is not redacted for secrets. A credential or private file content inside a message or tool
input can therefore be sent.

## Accounts and providers

One selected labeler labels all covered sessions. For example, choosing Claude Code to label both
clients sends Codex conversation text through Claude Code too. `configure` discloses this and asks
whether to continue before any call or save. Choose matching coverage and labeler to avoid
cross-provider labeling.

Calls use the client's current authentication and inherited environment. This may be a different
account from the one that recorded an older conversation. Atlas does not verify that they match.

The [required flags](LABELING.md#required-flags) disable tools and extra context, and skip regular
settings files: Claude Code uses `--setting-sources ""`; Codex uses `--ignore-user-config`. Labeling
therefore need not use the same configuration as an interactive conversation. The client's effective
routing, telemetry, logging, and provider policies still apply, including settings inherited through
environment variables. See the
[Claude Code environment reference](https://code.claude.com/docs/en/env-vars) and the
[Codex command reference](https://developers.openai.com/codex/cli/reference/).

## Local files and controls

Settings, the label cache, and optional report exports are stored locally. The
[label cache](CLI.md#label-cache) holds labels, pull request URLs, model names, timestamps, and
content hashes, not conversation text.

`run --no-label` makes no model calls. `configure` makes one small test call using a built-in
example, not your recorded conversations. `run --confirm` asks before making labeling calls, after
showing the estimate.

The clients' no-save flags stop labeling calls from being saved as local sessions. They do not
disable client logs or set provider retention or training policies; those depend on the client,
provider, account, and applicable settings.
