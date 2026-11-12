# Assumptions

What Atlas Nano assumes about the clients' files, and why.

## A session stored in several files is read from the longest copy

A session's key is its client and its file name without `.jsonl` or `.zst`. When several files share
a key, `ingest::load` keeps the copy with the most valid records and ignores the others. Malformed
lines do not count, so a damaged copy cannot displace a good one.

Claude Code writes the same session to more than one file in two cases:

- **A chat in two project folders.** The same `<session id>.jsonl` sits under two or three project
  folders, for example a worktree's and the main repository's, as when a chat is resumed from
  another folder.
- **A subagent under several parent chats.** The same `subagents/agent-<id>.jsonl` sits under two
  or three sessions, as when a chat is resumed or forked with its subagents. Whichever copy is
  kept, the subagent belongs to the session its records name in `sessionId`.

Codex was not seen writing duplicates.

**Why this is safe:** every extra copy seen was identical to the longest copy or a prefix of it, so
the longest copy holds every record. Measured on one machine in November 2026: 24 of 861 Claude Code
sessions had extra copies (26 extra files: 20 identical, 6 prefixes), and none of 1,169 Codex
sessions did.

**If it stops holding:** a shorter copy with records the longest lacks would lose those records,
silently.

## Parent rollouts for Codex subagents and Guardian reviews

A Codex chat keeps its thread ID across edits. Each saved version of the chat has its own rollout
ID. The first rollout uses the thread ID as its rollout ID. A subagent or Guardian review may
record only the parent thread ID, which all versions of that chat share.

**Assumption.** When the child names only the parent thread, Atlas Nano assumes the most recent
loaded rollout created at or before the child's start was the one that spawned it. It compares
the creation timestamps from the rollouts' own metadata so that copied history cannot affect the
choice. A recorded link to a specific rollout takes precedence. If the child's creation time is
unavailable, or the available times yield no single latest rollout, Atlas Nano uses the recorded
parent link.

For example, a chat starts on Monday with thread ID `T` and rollout ID `T`. On Friday, an edit
creates rollout `R` for the same thread `T`. A Saturday subagent records `parent_thread_id = T`.
Atlas Nano assigns it to `R`, so a report starting Friday includes the subagent and its spend with
the Friday edit. A Thursday subagent with the same parent thread ID belongs to `T` under this rule.

The parent thread ID and start times leave the actual parent rollout unknown. If the Monday
rollout `T` was still active and spawned the Saturday subagent, this rule would put the child and
its spend in Friday's report incorrectly.
