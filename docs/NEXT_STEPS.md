# Next steps

What is left to build, test, and decide for Atlas Nano. The [command line](CLI.md),
[report](REPORT.md), [labeling](LABELING.md), and [taxonomy](TAXONOMY.md) pages describe what is
built.

## To build

- An install script fetched with `curl`, and prebuilt binaries for macOS (Apple silicon and Intel)
  and Linux, published from the project's own public repository.
- A release workflow that builds every target when a version tag is pushed.

## To decide

- How many labeling calls run at once by default. It is 32, which has not been measured against each
  client's rate limits.
- Whether a `run` flag may override a configured value for one run.
- Whether users can supply their own taxonomy.
- How updates are installed, and whether macOS binaries need signing.

## To test

- A full labeling run on real history. The automated test drives a stand-in client, and `configure`
  has made one real call through each client; the parallel calls, the cache, and the report's
  category tables have not run against the real clients at scale.
- Rate limits at many calls at once, and the memory each client process takes.
- Chunking on a real session long enough to need it.
- Codex as the labeler for a whole run, and its usage and cost figures.
- Pull request mapping through the tool, and pull request numbers given without a URL across many
  repositories.
- The no-tools mode on subagents. Many Claude Code subagent files hold no model text, so they would
  be labeled from the task prompt alone.
- Run-to-run variation of one model on the same conversation.
- Moving the labeling instructions into the Codex instructions file.

## Manual review

No label has been checked by a person; every measurement so far is agreement between models. Before
the label lists are frozen, a person reviews a labeled sample of their own conversations. That
review also settles:

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
- A Codex thread edited into several rollouts is one conversation per rollout.

## Later

- Per-turn labeling, with per-turn rows in a second CSV keyed by `session_id` and `turn_index`.
- Fractional labels, replacing the chunk vote.
- A stripping mode that sends each tool's name and the first 100 characters of its input.
- Passing the parent's label as a hint when labeling a subagent with a thin task prompt.
- A price override file, for models the built-in table does not price.
