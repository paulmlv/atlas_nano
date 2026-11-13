# Report

What `atlas-nano run` prints, what its CSV holds, and how each number is defined.

## Printed report

The command prints its tables and exits, unless `--tui` asks for the
[interactive view](#interactive-view). Every category is shown both by conversation count and by
spend, each with its share. Numbers below are invented.

```text
atlas-nano · last 7 days · 142 conversations · 31 subagents · 12 reviews

Totals
  Spend (API-equivalent)  $412.30 (4 requests have no price and count as $0)
  Time    working 61h (9 turns with no recorded completion) · wall 190h
  Tokens  in 1.2M · cached 310M · cache-write 9.1M · out 2.4M
  Conversations 142 · subagents 31 · turns 1087

By type of work    Convs  Conv%  Subagents  Turns  Working  Wall  Tokens    Spend  Spend%
  Software           101    71%         27    840      49h  150h    280M  $371.00     90%
    Writing code      40    28%         12    310      18h   52h     98M  $140.10     34%
    ...
  Unlabeled            2     1%          0      3       2m    5m     40k    $0.12     <1%
  Automatic reviews    0     0%         12      0        -    1h    9.0M    $4.10      1%

By topic             (same columns, topics under their domains)

By client / model (top 10)  Requests  Tokens    Spend  Spend%
  claude / claude-opus-5-5       3210    250M  $301.20     73%
  ...
  (3 others)                       52    1.2M    $0.40     <1%

By project (top 10)  (same columns as the category tables)
By repo (top 10)     (same columns)

By day
  2026-11-09     $51.20  ██████████████
  2026-11-10    $110.42  ██████████████████████████████

Top 10 conversations by spend, subagents included
    Conversation               Started     Project    Spend
    Fix the failing export     2026-11-10  app/api   $48.10

Top 10 pull requests by spend (~ marks an amount split across several)
    Pull request                             Sessions    Spend
    https://github.com/owner/repo/pull/7            3  ~$31.07

Labeling run: 97 labeled, 45 from cache, 2 unlabeled · claude-haiku-5-5
  in 3.1M · out 12k tokens · API-equivalent $0.34
Prices checked 2026-11-03
```

- **Category tables:** labels are listed under their group or domain, largest spend first. Sessions
  without a label are in an `Unlabeled` row, and Codex safety reviews in an `Automatic reviews` row.
  The `Subagents` column counts subagents and reviews.
- **Empty rows** are left out of every table: a row with no tokens, so no spend, and in the tables
  with a `Convs` column, no conversations either. A row with tokens on a model without a price
  stays.
- **Shares** are whole percentages. `0%` means none; a share too small to round to 1% shows `<1%`.
- **Ranked tables:** By client / model, By project, By repo, the conversations, and the pull
  requests leave out empty rows, then show their 10 largest rows by spend and put the rest in one
  `(N others)` row, so the rows still sum to the total. The title says `top 10`. `configure` sets
  the count: 10, 20, 50, or `all`, which shows every row and drops `top` from the titles. The
  category tables are not limited. The pull requests' `(N others)` row leaves Sessions empty, since
  one session can work on several.
- **By client / model** counts model requests, since one session can use several models. A model
  without a price shows `no price`.
- **By project and By repo:** a chat's place is the folder it started in, and its subagents and
  reviews share it. Inside a git repository, the repo is the repository's folder name and the
  project is that name plus the path inside it: `~/Code/app/api` is project `app/api` in repo `app`.
  A Claude Code worktree (`.claude/worktrees/<name>`) counts as its repository, also once removed,
  and a Codex worktree while it exists. Outside a repository the project is the folder's name,
  `(temporary)` for a temporary folder, or `(no project)` for the home folder and Codex's folders
  for chats without a project, and the repo is `(no repo)`. A folder that no longer exists outside a
  repository, or a session that records none, has project and repo `(unknown)`; a removed temporary
  folder keeps `(temporary)` with repo `(unknown)`. Symbolic links are followed to the folder they
  name. The home folder itself is never a repository.
- **By day** puts each request's spend on the local day of the request, using the time zone's offset
  at that moment.
- **Top conversations** adds to each chat the spend of the subagents and reviews it started. A
  subagent whose chat was not loaded is not ranked. A session is shown by its title, else by the
  first line of its first request, else of the task an agent gave it. Control characters and
  text-direction marks in titles and folder names are shown as `�`.
- With `--no-label` the category tables and the labeling summary are left out.
- The pull request section appears only with pull request mapping on.

## Interactive view

`--tui` shows the report's tables in the terminal until `q`. It needs a terminal; with output sent
to a file or a pipe the run stops before labeling. A `--csv` file is still written first.

- **Header:** the period, spend, counts, time, and tokens of the totals block, with its notes on
  requests without a price and turns with no recorded completion.
- **Tabs:** one per table, in the printed order: type of work, topic, model, project, repo, day,
  conversations, and pull requests. `←`/`→`, `Tab`, or a number switches tabs.
- **Rows:** the same rows as the printed tables, empty rows left out, but every row: `top` does not
  apply, since the view scrolls. The labeling summary and the price date are not shown.
- **Open:** `Enter` on a line lists the conversations it holds, largest spend first, with only that
  line's part of their tokens and spend: the requests to that model or on that day, or the pull
  request's even split. A conversation's type of work is its chat's when the chat is in the line,
  else that of its first session that is. `Enter` on a conversation shows its sessions, the chat
  first and the others by spend, with labels, models, and each pull request once. `Esc` goes back.
- **Columns:** a column whose cells all hold one value moves above the table, so titles get the
  room. On a narrow screen, text and numbers that do not fit end in `…`.
- **Colors:** the configured theme, off as for the printed report. The selected line has a dark gray
  background, or reversed text with colors off.

## CSV

`--csv <path>` writes one row per session.

| Group    | Columns                                                                                 |
| -------- | --------------------------------------------------------------------------------------- |
| Identity | `client`, `session_id`, `parent_client`, `parent_session_id`, `kind`, `project`, `repo` |
| Time     | `started`, `ended` (UTC), `working_seconds`, `wall_seconds`                             |
| Counts   | `turns`, `untimed_turns`, `tool_calls`                                                  |
| Tokens   | `input`, `cached`, `cache_write`, `output`                                              |
| Spend    | `spend_usd`, `unpriced_requests`, `models`                                              |
| Labels   | `work`, `topic`, `prs`                                                                  |
| Labeling | `label_model`, `label_from_cache`                                                       |

`kind` is `primary`, `subagent`, or `review`. `models` lists the session's models separated by
spaces, and `prs` its pull requests as `role=URL` separated by `; `. The label columns are empty for
a session without a label. `parent_client` is the client of the parent, `claude` for a Codex chat in
a Claude Code scratchpad. `project` and `repo` are the root chat's; `repo` is empty outside a
repository and `(unknown)` when that cannot be told. `working_seconds` sums the turns with a known
working time; `untimed_turns` counts the others. A text field starting with `=`, `+`, `-`, `@`, a
tab, or a carriage return gets a leading `'`, so a spreadsheet shows it as text.

## Accounting

- **Turn:** one visible user request.
- **Spend:** each model request is priced at the provider's public API list prices. Plain input,
  cached input, cache writes, and output each use their own rate. Above a model's long-context
  boundary, its long rates apply to every token of the request. A request whose model has no price,
  or a Claude fast-mode request, costs $0 and is counted as unpriced, never guessed. Tool fees,
  batch or priority rates, and discounts are not included. This is API-equivalent spend, not what a
  subscription charges.
- **Prices:** the price table is built into the binary, and the report prints the date it was
  checked. Nothing is fetched from the network.
- **Each request counts once:** a model request copied into several sessions, as a fork copies its
  parent's history, counts only in the session that started first. Fork A into B after $10 and spend
  $2 more in B: A shows $10 and B shows $2.
- **Working time:** for each user request, the time from the request to the model's recorded
  completion, summed per session. It is unknown, not zero, for a turn with no recorded completion,
  an interrupted turn, a turn with work recorded after its completion, and a turn the user steered
  with another message while the model worked.
- **Wall time:** a session's first recorded timestamp to its last. Gaps count, and overlapping
  sessions add up in category totals.
- **Subagents:** each subagent's spend and time go to its own label. The conversation count and its
  share cover only chats the user started; subagents have their own count column, so a category can
  show spend with zero conversations.
- **Scratchpad chats:** a chat started in a Claude Code session's scratchpad,
  `/tmp/claude-<uid>/<project>/<session id>/scratchpad`, is a subagent of that session, which the
  path names, so its spend, period, and place follow that conversation. A chat another session
  started anywhere else, such as a `codex exec` review in a worktree, stays a conversation: nothing
  recorded says which session started it.
- **Times** are computed in UTC and converted to local time only for display.
