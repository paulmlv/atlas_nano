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

One selected labeler labels all covered sessions. For example, choosing Claude Code to label both
clients sends Codex conversation text through Claude Code too. `configure` discloses this and asks
whether to continue before any call or save. Choose matching coverage and labeler to avoid
cross-provider labeling.

Calls use the client's current authentication and inherited environment. This may be a different
account from the one that recorded an older conversation. Atlas does not verify that they match.

Labeling can use a different configuration from your interactive sessions because the
[required flags](LABELING.md#required-flags) skip the client's regular settings files. Environment
variables can still affect routing, telemetry, and logging. See the
[Claude Code environment reference](https://code.claude.com/docs/en/env-vars) and the
[Codex command reference](https://developers.openai.com/codex/cli/reference/).

## Local files and controls

Settings, the label cache, and optional report exports are stored locally. The
[label cache](CLI.md#label-cache) holds labels, pull request URLs, model names, timestamps, and
content hashes, not conversation text.

`run --no-label` makes no model calls. `configure` makes one small test call using a built-in
example, not your recorded conversations. `run --confirm` asks before making labeling calls, after
showing the estimate.

Labeling calls stay out of your local session history. Client logging, provider retention, and use
for training depend on the policies and settings for your client, provider, and account.
