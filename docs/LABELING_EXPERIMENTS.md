# Labeling experiments

Measurements behind the labeling design: what is sent to the labeling model, which model labels, and
how much model choice changes a report. They were taken on 2026-11-12 with Atlas Nano 0.1.0, Claude
Code 2.1.322 and Codex 0.167.0, on one person's local Claude Code and Codex history. Every call used
the shipped [prompt](../prompts/label.md), [labels](../prompts/labels.md) and
[required flags](LABELING.md#required-flags). All figures are aggregates; no conversation content
was recorded.

## What is sent

Each part's share of all characters in the local transcripts:

| Part                                                    | Share |
| ------------------------------------------------------- | ----- |
| Images returned by tools, stored as base64 text         | 37%   |
| Claude Code request logs                                | 16%   |
| Codex event stream, a second copy of tool output        | 9%    |
| Record envelope: ids, timestamps, field names           | 9%    |
| Injected context and session setup                      | 7%    |
| Tool result text                                        | 6%    |
| Copies of reviewed sessions inside Codex safety reviews | 5%    |
| Thinking, as signatures or encrypted text               | 4%    |
| Claude Code's structured copy of tool results           | 3%    |
| Codex compaction records                                | 2%    |
| Tool-call inputs                                        | 1%    |
| Images the user attached                                | 1%    |
| Model replies, user text, other agents' messages        | 0.3%  |

The stripping rule sends, for each session, the user's text, the model's replies, other agents'
messages, and each tool call's name and full input. Everything else is dropped, and Codex safety
reviews are not labeled. On this history that keeps about 1.5% of all characters, 82% of it
tool-call input. Without tool calls it keeps about 0.3%.

## Model comparison

100 conversations the user started were drawn at random: 36 Claude Code and 64 Codex, no subagent
sessions. Conversations with no typed request, and the 5% too long for one call, were left out. The
median conversation is 19K characters after stripping and the largest 266K. Each was labeled at
medium effort by Claude Opus 5.5 and Claude Haiku 5.5 through Claude Code, and by GPT-6 Astra and
GPT-6 Luna through Codex. No call failed.

Opus is the reference. Agreement with Opus is not correctness: nobody checked Opus's labels.

| Agreement with Opus 5.5, out of 100 | Astra | Haiku | Luna |
| ----------------------------------- | ----- | ----- | ---- |
| Type of work                        | 85    | 89    | 62   |
| Topic                               | 83    | 81    | 58   |
| Both labels                         | 72    | 72    | 37   |

- Labeling the same 100 again, Opus keeps 94 of its types of work and 90 of its topics, and Haiku 89
  and 88. Differences of a few points between models may be noise.
- At least three of the four models agree on 86 conversations for type of work and 82 for topic.
- Most disagreements are between neighbouring labels: Version control against the change it delivers
  (Writing code, Refactoring and maintenance, Reviewing code), and Backend and APIs against Data
  storage and pipelines, Developer tooling and CI, or Architecture and code quality.

## Effect on a report

The same 100 conversations, aggregated once by Opus's labels and once by Astra's. Spend is the
API-equivalent cost of each conversation's own requests at list prices. The ten most expensive
conversations hold 43% of it. Five conversations have no usage record and count as no spend.

| Level                  | Share of spend that changes category |
| ---------------------- | ------------------------------------ |
| Type of work, by group | 5%                                   |
| Type of work, by label | 10%                                  |
| Topic, by domain       | 7%                                   |
| Topic, by label        | 18%                                  |

- No label's conversation count differs by more than 5 out of 100.
- Group and domain totals are close: Software is 82% of spend under Opus and 84% under Astra, and
  Engineering 70% and 73%.
- Spend per label moves more, because a few long, expensive conversations mix several kinds of work.
  Data storage and pipelines is 8% of spend under Opus and 13% under Astra; AI agents and tools is
  13% and 9%. Assigning a conversation fractionally to several labels would remove most of this.

## Cost of labeling

| Model       | API-equivalent cost of the 100 calls | Median seconds per call |
| ----------- | ------------------------------------ | ----------------------- |
| Opus 5.5    | $16.10                               | 4.1                     |
| Haiku 5.5   | $0.58                                | 4.2                     |
| GPT-6 Astra | $17.33                               | 8.6                     |
| GPT-6 Luna  | $0.17                                | 6.3                     |

- Claude Code reports its cost. Codex reports none, so its cost is priced from the token counts with
  the built-in price table.
- Content came to about 2.4 characters per token on Claude and 3.7 on Codex, not the 4 often
  assumed, because tool input tokenizes poorly.
- Claude Code writes each prompt to a one-hour cache at twice the input price, so Haiku came to
  about $0.25 per million input tokens against a list input price of $0.10. A second run within the
  hour reads that cache and costs a fraction: $0.07 for Haiku and $1.37 for Opus.
- These are API-equivalent figures. On a subscription they are usage, not a bill.

## Required flags

A call that labels a one-line conversation takes about 36,400 input tokens with plain `claude -p`
and about 890 with the required flags. Through Codex it takes about 22,600 with the user's
configuration loaded and 5,400 with the required flags. Neither figure includes the labeling
instructions.

- With the required flags Claude Code still sends the user's email and an environment block with the
  working directory, platform and date, and no instruction files, memory or skills. What Codex's
  remaining tokens hold is not identified.
- `claude -p --no-session-persistence` and `codex exec --ephemeral` wrote no session file across the
  900 labeling calls of these measurements.
- `claude --bare` answers "Not logged in" on a subscription, so it cannot replace the required flags
  to leave out memory and skills.

## Pull request mapping

The same 100 conversations, 84 with a known repository, were labeled by Opus 5.5, Haiku 5.5, and
GPT-6 Astra in one call that also returned the pull requests. No call failed.

- Each model found a pull request in 57 conversations: Opus returned 162 pull requests, Haiku 168
  and Astra 172.
- The models agree. Haiku and Opus return the same set in 96 conversations and give the same role to
  148 of the 161 pull requests they share. Haiku and Astra return the same set in 96, Opus and Astra
  in 95.
- Every pull request that only one model returns, compared with another, is Mentioned only.
- Of the pull requests that receive cost, Haiku and Opus share 60; Haiku alone has 7 and Opus
  alone 3.
- Opus returned no pull request whose number is absent from the text. Haiku and Astra each returned
  4, all Mentioned only, so none receives cost. No model returned a repository found neither in the
  text nor in the session.

## Limits

- One person's history, mostly software work: about 80% of the sample's spend is in the Software
  group, so most taxonomy labels were barely exercised.
- No label was checked by a person.
- Run-to-run variation was measured for Opus and Haiku only, once each.
- The pull request sample spans very few repositories.
- Subagent sessions and conversations too long for one call were not part of the sample.
