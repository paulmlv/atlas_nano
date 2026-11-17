# Taxonomy

Atlas Nano gives each conversation one type of work and one topic, chosen by a labeling model from
fixed lists. This page describes how labeling works and the pull request mapping. The lists and
their definitions are in [prompts/labels.md](../prompts/labels.md), and the rules the model follows
are in the [labeling prompt](../prompts/label.md). The [manual review](#manual-review) checks the
labels against a person's choices.

## How labeling works

- The labeling model is Claude Haiku through Claude Code, or GPT-6 Luna through Codex. No other
  model labels.
- Labels are fixed. The model picks exactly one type of work and exactly one topic per conversation.
- Each type of work belongs to a group and each topic to a domain. Reports roll up by group and
  domain; the model picks a label, never a group or domain.
- How the model decides between labels (mixed conversations, work versus personal, Research versus
  Planning and design, Version control, topics in a codebase) is in the rules of the
  [labeling prompt](../prompts/label.md).
- Every session is labeled, including subagent sessions and sessions with no request typed by the
  user. The only sessions never labeled are automatic reviews, which Codex records as safety
  reviews. A session with no text to send stays unlabeled.
- Project names stay out of the taxonomy; the per-project breakdown comes from folder and repository
  metadata.
- A conversation is labeled as a whole. There is no per-turn label and no split across labels.

## Pull request mapping

- On by default; configuration asks whether to turn it off.
- Local files only, and no code searches for pull requests. The same model call that assigns the
  type of work and topic also returns every pull request mentioned in the conversation, with its
  role. The model's answer is used as returned, with no check against the text.
- The model reads the stripped conversation, so it does not see tool output. It learns of a pull
  request from the user's messages, the assistant's replies, and the tool commands.
- A pull request is always identified by its full URL (`https://github.com/owner/repo/pull/7`),
  never a bare number, in the model's answer and the report. The prompt gives the session's
  repository so the model can resolve a bare number; with no known repository it omits the pull
  request.
- Roles, one per pull request, defined in the [labeling prompt](../prompts/label.md): Contributed to
  and Reviewed attribute the conversation's cost to the pull request; Mentioned only attributes
  nothing.
- A conversation that worked on several pull requests splits its cost evenly among them, and the
  report marks split amounts as estimates.

## Manual review

On 2026-11-17, GPT-6.2 Astra and Claude Opus 5.5 labeled the same 100 conversations, sampled by
spend from one person's history. That person reviewed 20 of them at random without knowing which
model gave which labels, then revisited six cases after seeing the models.

| Accepted, of 20 | Pairs (blind) | Pairs | Topics | Types of work |
| --------------- | ------------: | ----: | -----: | ------------: |
| GPT-6.2 Astra   |            15 |    16 |     18 |            16 |
| Claude Opus 5.5 |            16 |    16 |     18 |            18 |

Both models had 80% of their pairs and 90% of their topics accepted. Twenty conversations are too
few to rank them.
