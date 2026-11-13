# Next steps

What is left to test and decide for Atlas Nano. The [command line](CLI.md), [report](REPORT.md),
[labeling](LABELING.md), and [taxonomy](TAXONOMY.md) pages describe what is built.

## To decide

- Whether a `run` flag may override a configured value for one run.
- Whether users can supply their own taxonomy.
- Whether the interactive view sorts, filters, or changes the period without leaving.
- How updates are installed, and whether macOS binaries need signing.

## To test

- A full labeling run on real history. The automated test drives a stand-in client, and `configure`
  has made one real call through each client; the parallel calls, the cache, and the report's
  category tables have not run against the real clients at scale.
- Rate limits at many calls at once, Codex with many calls at once, and the
  [limit on calls at once](LABELING_EXPERIMENTS.md#calls-at-once) on machines other than one Mac.
- Chunking on a real session long enough to need it.
- Codex as the labeler for a whole run, and its usage and cost figures.
- Pull request mapping through the tool, and pull request numbers given without a URL across many
  repositories.
- The no-tools mode on subagents. Many Claude Code subagent files hold no model text, so they would
  be labeled from the task prompt alone.
- Run-to-run variation of Astra and Luna, and of Opus and Haiku on more than the one
  [measurement](LABELING_EXPERIMENTS.md#model-comparison) so far.
- Moving the labeling instructions into the Codex instructions file.

## Manual review

No label has been checked by a person; every [measurement](LABELING_EXPERIMENTS.md) so far is
agreement between models. Before the label lists are frozen, a person reviews a labeled sample of
their own conversations. That review also settles:

- whether to keep the reworded type-of-work definitions, which did not raise agreement;
- the split between Backend and APIs and Data storage and pipelines;
- the Translating and Creative work labels, which have not been tried on real conversations.

## Known limits

- A request counts once only when its copies keep its identity: the response ID for Claude Code, and
  the turn, time, and running token total for Codex. A copy that changed any of these counts again.
- A Codex code-review session whose stripped text is only a verdict gives the model no subject, so
  its topic is a guess. Keeping the review target (branch, base, or changed file names) in the text
  would fix it.
- A long conversation that mixes several kinds of work gets one label, which moves spend between
  labels. Fractional labels would remove most of this.
- Every run reads every session file, which takes a few seconds on a large history. An index of
  parsed sessions would make a short period faster.
- Claude Code sessions recorded only in its typed history, with no transcript, are not read.
- Codex code reviews run with `codex exec` (seen in versions 0.166 and 0.167) record no usage, so
  their spend is missing and the run warns. The commands they run appear only in `item_completed`
  events, so they are not counted as tool calls either.
- The missing-usage warning looks at whole sessions: one where some replies have usage and others do
  not is not flagged. It also needs some replies to be read: a session whose replies and usage both
  moved to records Atlas Nano does not read gets no warning.
- Codex rollouts migrated by Codex 0.149 can carry their messages and replies only in
  `item_completed` events, which are not read, so such a session has no text to label.
- A Codex thread edited into several rollouts is one conversation per rollout.

## Later

- Per-turn labeling, with each session's turns nested in the JSON, and per-turn rows in a second CSV
  keyed by `session_id` and `turn_index`.
- Fractional labels, replacing the chunk vote.
- A stripping mode that sends each tool's name and the first 100 characters of its input.
- Passing the parent's label as a hint when labeling a subagent with a thin task prompt.
- A price override file, for models the built-in table does not price.
- A check against Claude Code's own cost total, its `cost-state` records, to catch spend that is
  missed or priced wrong.
