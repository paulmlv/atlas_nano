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
- Chunking end to end in `atlas-nano run`. The manual comparison below used chunks for 24
  conversations.
- Codex as the labeler for a whole run, and its usage and cost figures.
- Pull request mapping through the tool, and pull request numbers given without a URL across many
  repositories.
- The no-tools mode on subagents. Many Claude Code subagent files hold no model text, so they would
  be labeled from the task prompt alone.
- Run-to-run variation of Astra and Luna, and of Opus and Haiku on more than the one
  [measurement](LABELING_EXPERIMENTS.md#model-comparison) so far.
- Moving the labeling instructions into the Codex instructions file.

## Manual review

On 2026-11-17, GPT-6.2 Astra and Claude Opus 5.5 each labeled the same 100 conversations at high
effort. The sample came from one person's history (44 Claude Code and 56 Codex conversations), drawn
without replacement using Atlas Nano's recorded conversation spend, including linked subagents and
reviews, as the weight.

The reviewer assessed 20 randomly selected conversations. Each case showed a conversation summary
and two label pairs in random order. After seeing which model chose each pair, the reviewer
revisited the six cases where they had accepted one pair or neither.

| Model           | Pairs accepted blind | Final pair matches | Final topic matches | Final type-of-work matches |
| --------------- | -------------------: | -----------------: | ------------------: | -------------------------: |
| GPT-6.2 Astra   |                15/20 |              16/20 |               18/20 |                      16/20 |
| Claude Opus 5.5 |                16/20 |              16/20 |               18/20 |                      18/20 |

Both models matched 80% of the reviewer's final pairs and 90% of the topics. Opus matched two more
type-of-work labels. The model comparison is tentative given the 20 reviewed conversations from one
person's spend-weighted sample.

### Remaining manual checks

- Compare the original and reworded type-of-work definitions with a reviewer.
- Check the split between Backend and APIs and Data storage and pipelines.
- Check Translating and Creative work on real conversations.

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
- Atlas can flag a missing measurement when it recognizes a reply or a usage record. If a client
  moves both to an unfamiliar record type, the session can still go unflagged.
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
