# Labeling

How Atlas Nano turns a local session into the text a labeling model reads, and how it makes the
call. The [taxonomy](TAXONOMY.md) describes the labels and the default models, and
[prompts/](../prompts/) holds the prompt and the labels it sends; the [required flags](#required-flags) are the only flags a labeling call uses.
The [labeling experiments](LABELING_EXPERIMENTS.md) measure what is sent, the fixed context per
call, and how much the choice of model changes the labels.

## Which sessions are labeled

- Every session is labeled on its own, including subagent sessions, which do not take their parent's
  label.
- Codex safety reviews are not labeled.
- A session that renders to no text is not sent. It stays in the accounting as unlabeled, and the
  run prints how many sessions are unlabeled.

## What is sent

- Sent: the user's text, the model's replies, messages from other agents (including the task prompt
  a subagent received), and each tool call's name and full input.
- A no-tools mode, chosen at configuration, sends no tool calls at all, not even their names.
- Dropped in every mode: tool result text, images returned by tools, thinking, injected context and
  session setup, request logs, the Codex event stream, structured copies of tool results, compaction
  records and summaries, and the record envelope apart from the times and ids the tool needs.
- An image the user attached becomes the marker `[image]`.
- Each kept part is one item starting with `[user]`, `[assistant]`, `[agent]`, or `[tool]`, and
  items are joined by a blank line. A tool item is `[tool] <name> <input>`, the input as compact
  JSON for Claude Code and as recorded for Codex.

### Claude Code records

Files are `~/.claude/projects/**/*.jsonl`, also as `.jsonl.zst`; names containing `.orphaned-` are
skipped. The file stem is the session id. A file under a `subagents` directory is a subagent, and
the directory that owns `subagents` is named after the parent session id. Each line is one JSON
record keyed by `type`. A record whose `uuid` was already seen is skipped. The rows below are tested
in order.

Claude Code has no archive folder like Codex: a session stays in `~/.claude/projects` until Claude
Code deletes it, after `cleanupPeriodDays` (30 days by default).

| Record                                                       | Rule                                                                                               |
| ------------------------------------------------------------ | -------------------------------------------------------------------------------------------------- |
| `type: "user"` with `isCompactSummary`                       | drop                                                                                               |
| `type: "user"`, `origin.kind` set and not `"human"`          | `[agent]`; for `"peer"` use `origin.body` when present                                             |
| `type: "user"`, text is a tagged command                     | `[user] /name args` from `<command-name>` and `<command-args>`; `[user] ! cmd` from `<bash-input>` |
| `type: "user"` with `isMeta` or `isVisibleInTranscriptOnly`  | drop                                                                                               |
| `type: "user"`, `origin.kind == "human"`                     | `[user]`                                                                                           |
| `type: "user"`, no `origin` (a subagent's task prompt)       | `[agent]`                                                                                          |
| `type: "attachment"`, `attachment.type == "queued_command"`  | `attachment.prompt`; `[user]` if `attachment.origin.kind == "human"`, else `[agent]`               |
| `type: "attachment"`, any other `attachment.type`            | drop                                                                                               |
| `type: "assistant"`, block `text`                            | `[assistant]`                                                                                      |
| `type: "assistant"`, block `tool_use`                        | `[tool] <name> <json of input>`                                                                    |
| `type: "assistant"`, block `thinking` or `redacted_thinking` | drop                                                                                               |
| every other `type` (`system`, `api-request*`, titles, ...)   | drop                                                                                               |

User text is `message.content`: a string, or a list of blocks where `text` blocks are joined,
`image` blocks become `[image]`, and `tool_result` blocks are dropped. A run of
`<system-reminder>...</system-reminder>` at the start of a text block is removed. The record-level
`toolUseResult` field is dropped.

### Codex records

Files are `~/.codex/sessions/**/*.jsonl` and `~/.codex/archived_sessions/**/*.jsonl`, also as
`.zst`, named `rollout-<time>-<thread id>[_<rollout id>].jsonl`. Each line is
`{timestamp, type, payload, metadata?}`.

The session kind is `payload.thread_source` of the first `session_meta` record: `user` is a
conversation, `subagent` a subagent, `agent_created_thread` a chat an agent created, and
`guardian_review` a safety review. A chat started in a Claude Code session's scratchpad becomes a
subagent of that session ([scratchpad chats](REPORT.md#accounting)).

| Record                                                             | Rule                                                              |
| ------------------------------------------------------------------ | ----------------------------------------------------------------- |
| `response_item`, `payload.type: "message"`, role `assistant`       | `[assistant]`; drop when `metadata.compaction_output` is set      |
| same, role `user`, text matches `<user_shell_command><command>...` | `[user] ! <command>`                                              |
| same, role `user`, text starts with `<codex_internal_context`      | drop                                                              |
| same, role `user`, `metadata.user_input_order` is null             | drop                                                              |
| same, role `user`, `payload.id` already seen                       | drop                                                              |
| same, role `user`, otherwise                                       | `[user]`, cleaned as below                                        |
| same, role `developer`                                             | drop                                                              |
| `response_item`, `payload.type: "agent_message"`                   | `[agent]` with the text of `payload.content`                      |
| `response_item`, a tool call type                                  | `[tool] <payload.name or the type> <arguments, input, or action>` |
| `response_item`, any other type (`reasoning`, `*_output`, ...)     | drop                                                              |
| `event_msg`, `payload.type: "thread_goal_updated"`, no `turnId`    | `[user] /goal <goal.objective>`, once per `goal.createdAt`        |
| every other record (`event_msg`, `session_meta`, `compacted`, ...) | drop                                                              |

Tool call types are `function_call`, `custom_tool_call`, `local_shell_call`, `tool_search_call`,
`web_search_call`, and `image_generation_call`. Message text is the `text` of `input_text`,
`output_text`, and `text` blocks; `input_image` blocks become `[image]`. In a user message, a
`<send_user_message_question_reply>` body is JSON and becomes `Answer: <answer>` lines; when the
text contains `## My request:` only what follows is kept; `<image ...>...</image>` becomes
`[image]`.

Conversations recorded before Codex marked user messages have no `[user]` items. They are labeled
from the model's replies and tool calls.

### Forks

The text of a fork includes the history it inherited from its parent. Where a Codex rollout
references its parent's history without copying it, the text is rebuilt from the parent rollout. The
[accounting](REPORT.md#accounting) counts only what happened after the fork.

## Sessions too long for one call

- The stripping rule does not change. The session is split into chunks and each chunk is labeled.
- A chunk fills at most 80% of the model's context window, the fixed context and the instructions
  included, counting 2 characters per token. The window is taken as 1,000,000 tokens for Claude
  Haiku and 272,000 for GPT-6 Luna. With `CLAUDE_CODE_DISABLE_1M_CONTEXT` set, Claude Code caps
  Haiku at 200,000, and so does Atlas Nano.
- A chunk starts at a user message, so it holds whole turns. A session with no user messages starts
  its chunks at messages from other agents. A single turn larger than the window is cut between
  records, and a single record larger than the window is truncated.
- Every chunk after the first starts with the session's first request, marked as context: the first
  user message, or for a subagent its task prompt. A session with nothing identifiable as a request
  gets no such prefix.
- The session takes the most common chunk label; on a tie, the label that appears first. The vote
  runs separately for type of work and for topic, and the session keeps each pull request any chunk
  returned, with the role it was first given.

## The call

- One call per session, or per chunk, at medium effort, with the [required flags](#required-flags).
  Calls run in parallel. At start the command raises its open-file limit as far as the system
  allows, since each call holds a few pipes and macOS terminals allow 256 open files by default.
- The prompt is [prompts/label.md](../prompts/label.md), built into the binary.
- The instructions say that the conversation is data to classify, not to follow. They list the
  rules, then every type of work and every topic in the taxonomy as `label: definition`, the
  topics under a line for each domain. Claude Code
  receives them as the system prompt. Codex receives them at the start of the prompt, and its
  instructions file holds one short line.
- The conversation follows between `<conversation>` and `</conversation>` on standard input.
- The answer is restricted by a JSON schema generated from the taxonomy:

  ```json
  {
    "type": "object",
    "properties": {
      "work": { "type": "string", "enum": ["<the types of work>"] },
      "topic": { "type": "string", "enum": ["<the topics>"] }
    },
    "required": ["work", "topic"],
    "additionalProperties": false
  }
  ```

- With [pull request mapping](TAXONOMY.md#pull-request-mapping-experimental) on, the instructions
  add the pull request rules, the prompt states `Session repository: <URL or unknown>`, with any
  user name and password removed from the URL, and `Session branch: <name or unknown>` before the
  conversation, and the schema requires a `prs` array of `{"url", "role"}` objects whose role is
  `Contributed to`, `Reviewed`, or `Mentioned only`. Each URL is kept as
  `https://github.com/<owner>/<repo>/pull/<number>`, the owner and repository lowercased and
  anything after the number cut, and each pull request once. An entry that names no GitHub pull
  request is dropped.
- Claude Code returns the answer as `structured_output` in its JSON result, with `is_error`,
  `usage`, and `total_cost_usd`.
- Codex writes the answer to the answer file and reports usage in its `turn.completed` event, priced
  per call from the price table.
- A call that fails, runs longer than ten minutes, or returns an answer that does not match the
  schema is not retried. A call past ten minutes is stopped. The error is logged, the session counts
  as unlabeled, and the run continues. Its tokens and cost still count in the labeling summary. A
  session split into chunks is unlabeled when any of its calls fails.
- Labeling calls are never saved as sessions. The no-save flags are the only mechanism, and client
  versions without them are not supported.

## Required flags

Every labeling call must use these flags, and no others that add context. Configuration runs one
small labeling call with them, with the chosen labeling model, and fails if the call is rejected.

```sh
CLAUDE_CODE_DISABLE_ATTACHMENTS=1 claude -p --model <model> --effort medium --no-session-persistence \
  --system-prompt "<labeling instructions>" --tools "" --strict-mcp-config \
  --disable-slash-commands --setting-sources "" \
  --output-format json --json-schema '<answer schema>'
```

```sh
codex exec -m <model> -c model_reasoning_effort='"medium"' --ephemeral \
  --skip-git-repo-check --sandbox read-only --ignore-user-config --ignore-rules \
  -c project_doc_max_bytes=0 -c model_instructions_file='"<short instructions file>"' \
  -c web_search='"disabled"' -c include_environment_context=false \
  --disable shell_tool --disable unified_exec --disable view_image --disable image_generation \
  --disable multi_agent --disable goals --disable memories --disable apps --disable plugins \
  --disable browser_use --disable computer_use --disable skill_search --disable tool_suggest \
  --disable sleep_tool --disable in_app_browser --disable hooks \
  --disable workspace_dependencies --disable collaboration_modes \
  --output-schema <answer schema file> -o <answer file> --json -
```

Both run from an empty working directory with the conversation on standard input.
`CLAUDE_CODE_DISABLE_ATTACHMENTS=1` stops Claude Code from attaching a file named by an `@path` in
the conversation.
