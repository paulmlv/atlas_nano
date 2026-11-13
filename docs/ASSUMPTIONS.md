# Assumptions

What Atlas Nano assumes about the clients' files, and why.

## A session stored in several files is read from the longest copy

When several files share a client and file name, `ingest::load` keeps the copy with the most valid
records. Claude Code makes such copies when a chat is resumed from another folder, or forked with
its subagents; a copied subagent still belongs to the session its records name.

**Why it is safe:** in a check in November 2026, every extra copy seen was identical to the longest
or a prefix of it. Only Claude Code sessions had copies; Codex sessions had none. A shorter copy
with records the longest lacks would lose them silently.

## A Codex child belongs to the latest version of its parent thread

Each edit of a Codex chat is a new rollout in the same thread. A subagent or Guardian review that
records only the parent thread is assigned to the latest rollout of that thread created at or before
the child, by the rollouts' own creation times. A recorded rollout link takes precedence.

**Risk:** if an older rollout of the thread was still active and spawned the child, the child and
its spend land with the newer version, possibly in the wrong period.
