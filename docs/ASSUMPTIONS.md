# Assumptions

What Atlas Nano assumes about the clients' files, and why.

## A session stored in several files is read from the longest copy

A session's key is its client and its file name without `.jsonl` or `.zst`. When several files share
a key, `ingest::load` keeps the copy with the most records and ignores the others.

Claude Code writes the same session to more than one file in two cases:

- **A chat in two project folders.** The same `<session id>.jsonl` sits under two or three project
  folders, for example a worktree's and the main repository's, as when a chat is resumed from
  another folder.
- **A subagent under several parent chats.** The same `subagents/agent-<id>.jsonl` sits under two
  or three sessions, as when a chat is resumed or forked with its subagents.

Codex was not seen writing duplicates.

**Why this is safe:** every extra copy seen was identical to the longest copy or a prefix of it, so
the longest copy holds every record. Measured on one machine in November 2026: 24 of 861 Claude Code
sessions had extra copies (26 extra files: 20 identical, 6 prefixes), and none of 1,169 Codex
sessions did.

**If it stops holding:** a shorter copy with records the longest lacks would lose those records,
silently.
