# Privacy

Atlas Nano reads local Claude Code and Codex session files. It has no servers or telemetry of its
own.

## What is sent

Your selected client sends [conversation text](LABELING.md#what-is-sent) to the labeling provider.
Credentials or private file contents in that text are sent as written. Tool inputs are included in
full by default. You can turn off tool calls in `configure`. With
[pull request mapping](LABELING.md#the-call) on, the provider also receives the repository URL and
branch when known.

## Accounts and providers

You choose one client to label the sessions you cover. If you cover only Codex sessions and choose
Claude Code as the labeler, their text goes through Claude Code. Whenever the labeler would read
another client's sessions, `configure` asks you to confirm before it makes its test call or saves
your settings.

To keep the two clients' conversations separate, cover only the client you use for labeling. The
report then excludes the other client's sessions and spend. To include both clients' spend without
making model calls, cover both and pass `--no-label` on every `run`. A plain `run` uses the saved
labeler without asking again.

Calls use the client's current authentication and routing. If Claude Code now uses a company
gateway, it sends older Claude Code conversations there too. Atlas Nano does not verify that the
account or provider matches the original conversation.

Labeling can use a different configuration from your interactive sessions because the
[required flags](LABELING.md#required-flags) skip the client's regular settings files. Environment
variables can still affect routing, telemetry, and logging. See the
[Claude Code environment reference](https://code.claude.com/docs/en/env-vars) and the
[Codex command reference](https://developers.openai.com/codex/cli/reference/).

## Local files and controls

Settings, the label cache, and optional report exports are stored locally. The
[label cache](CLI.md#label-cache) holds labels, pull request URLs, model names, timestamps, and
content hashes, not conversation text.

`configure` makes one small test call using a built-in example, not your recorded conversations.
`run --confirm` asks before making labeling calls, after showing the estimate.

Labeling calls stay out of your local session history. Client logging, provider retention, and use
for training depend on the policies and settings for your client, provider, and account.
