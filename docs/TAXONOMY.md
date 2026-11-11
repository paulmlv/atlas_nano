# Taxonomy

Atlas Nano gives each conversation one type of work and one topic, chosen by a labeling model from
the fixed lists below. This page is the contract for those labels, the rules the model follows, and
the experimental pull request mapping. The labels have not been checked by hand.

## Rules

- The labeling model is Claude Haiku through Claude Code, or GPT-6 Luna through Codex. No other
  model labels.
- Labels are fixed. The model picks exactly one type of work and exactly one topic per conversation.
- A mixed conversation takes the label for most of the assistant's work, judged by what its tool
  calls and replies do. The opening request and the number of user turns only break ties.
- Groups and domains are for report roll-ups; the model picks a label, never a group or domain.
- Work versus personal follows from the topic's domain: Personal is one domain, every other domain
  is work. A conversation is work if it concerns a job, a business, or a codebase, and Personal if
  it concerns the user's own life. With no sign either way it goes to Personal / General knowledge.
- Research versus Planning and design: Research when the answer exists in the outside world and has
  to be found; Planning and design when the answer is a choice about the user's own work. Reading
  one given page, paper, or repository is Reading a document or Understanding code, not Research.
- Version control applies when the Git work is the point: cleaning up branches, retargeting, merging
  or rebasing with conflict resolution, releasing. When commits and pull requests deliver a change
  the conversation itself makes, label the change, such as Writing documentation or Refactoring and
  maintenance.
- In a codebase, the topic is the part of the system the changed, reviewed, or explained code
  belongs to (server and API, data and storage, interface, build and CI, infrastructure). It is not
  what the product is about and not the quality the user asks for, such as correctness, simplicity,
  or clarity. Software that serves, meters, records, or analyzes AI assistants still takes the topic
  of its part, such as Backend and APIs or Data storage and pipelines.
- Tests, linters, and type checks run only to verify other work do not set the topic. Version
  control work that changes no code of its own (cleaning up branches, retargeting or merging pull
  requests) takes Developer tooling and CI unless one code subject dominates.
- A type of work is never repeated as a topic: a conversation that writes documentation takes the
  topic of what is documented, and one that analyzes data takes the topic of what the data is about.
- Every session is labeled, including subagent sessions and sessions with no request typed by the
  user. The only sessions never labeled are automatic reviews, which Codex records as safety
  reviews. A session with no text to send stays unlabeled.
- Project names stay out of the taxonomy; the per-project breakdown comes from folder and repository
  metadata.
- A conversation is labeled as a whole. There is no per-turn label and no split across labels.

## Type of work (26 labels)

| Group     | Label                            | Definition                                                                                                                       |
| --------- | -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Software  | Writing code                     | Adding or changing behavior that works as designed because the user wants it different                                           |
| Software  | Debugging                        | Tracing an error, failing check, or wrong result to its cause and fixing it, including bugs found in review                      |
| Software  | Refactoring and maintenance      | Restructuring, speeding up, upgrading, or tidying code without new behavior                                                      |
| Software  | Testing                          | Writing or running tests, or checking that something behaves as intended                                                         |
| Software  | Reviewing code                   | Judging a diff, pull request, branch, or existing code, including whether it is still needed                                     |
| Software  | Understanding code               | Having a codebase or change explained                                                                                            |
| Software  | Writing documentation            | Writing or updating READMEs, guides, and comments that describe software                                                         |
| Software  | Version control                  | Commits, branches, merges, pull requests, releases, and branch cleanup                                                           |
| Software  | Setup, deployment and operations | Installing, configuring, deploying, running, or monitoring software, including an assistant's own settings and instruction files |
| Software  | Orchestrating agents             | Directing subagents or automated pipelines that do the work                                                                      |
| Knowledge | Planning and design              | Deciding how to do something before doing it                                                                                     |
| Knowledge | Research                         | Gathering and comparing several outside sources to answer an open question                                                       |
| Knowledge | Learning                         | Being taught how a concept or a tool works, or working through exercises                                                         |
| Knowledge | Reading a document               | Summarizing or extracting from a document, page, or video                                                                        |
| Knowledge | Analyzing data                   | Computing figures, tables, or charts from data                                                                                   |
| Knowledge | Writing prose                    | Drafting messages, posts, or other text that is not software documentation                                                       |
| Knowledge | Editing and reviewing writing    | Improving, proofreading, or critiquing existing text                                                                             |
| Knowledge | Brainstorming                    | Generating ideas, names, or options                                                                                              |
| Knowledge | Translating                      | Converting text between languages                                                                                                |
| Knowledge | Creative work                    | Fiction, poetry, images, audio, or video made for their own sake                                                                 |
| General   | Seeking advice                   | Asking for a recommendation on a personal, financial, or business choice                                                         |
| General   | How-to and troubleshooting       | Step-by-step help using or fixing an app, device, service, or account that is not own code                                       |
| General   | Doing a task for me              | An errand outside software work: operating an app, filling a sheet, sending a message                                            |
| General   | Looking something up             | Finding a specific fact, file, conversation, or item                                                                             |
| General   | Casual chat                      | Greetings, jokes, and one-line tests with no task                                                                                |
| Any       | Other                            | None of the above fits                                                                                                           |

## Topic (12 domains, 49 topics)

| Domain                | Topic                           | Definition                                                                                             |
| --------------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Engineering           | Backend and APIs                | Servers, services, request handling, and integrations                                                  |
| Engineering           | Frontend and UI                 | Web pages, dashboards, and interface code                                                              |
| Engineering           | Mobile apps                     | Phone and tablet applications                                                                          |
| Engineering           | Other platforms                 | Desktop applications, embedded devices, and game development                                           |
| Engineering           | Data storage and pipelines      | Schemas, queries, file formats, and jobs that move or transform data                                   |
| Engineering           | Infrastructure and deployment   | Hosting, networking, containers, and releases to production                                            |
| Engineering           | Developer tooling and CI        | Editors, linters, package managers, build systems, CI, and Git branch housekeeping                     |
| Engineering           | Architecture and code quality   | How a codebase is split into modules and layers, and conventions that span it, not one part of it      |
| Engineering           | Engineering process and quality | Team policy for planning, reviewing, testing, and releasing, not one review or merge                   |
| Engineering           | Security                        | Authentication, secrets, vulnerabilities, and access control                                           |
| Engineering           | Computer science concepts       | Algorithms, languages, and how computing works, apart from any named tool or service                   |
| Data and AI           | Statistics and analytics        | Statistics, metrics, charts, what the numbers mean, and code that computes them                        |
| Data and AI           | Machine learning                | Training, evaluating, or applying models built by the user                                             |
| Data and AI           | AI models and benchmarks        | Comparing models, their releases, capabilities, and research papers                                    |
| Data and AI           | AI agents and tools             | Using, configuring, prompting, or paying for AI assistants and agents, or building an agent's own loop |
| Product               | Product strategy                | What to build, for whom, and in what order                                                             |
| Product               | User research                   | Understanding customers, their needs, and feedback                                                     |
| Product               | Specifications                  | Requirements, proposals, and feature definitions                                                       |
| Design                | Visual and interface design     | Layout, typography, and how something looks and feels                                                  |
| Design                | Branding and naming             | Names, identity, and tone for a product or company                                                     |
| Business and strategy | Markets and competitors         | Industries, market size, trends, rival products, and specific companies                                |
| Business and strategy | Startups and fundraising        | Founding, pitching, and investors                                                                      |
| Business and strategy | Pricing                         | What to charge and the economics of a product                                                          |
| Finance               | Accounting                      | A company's books, ledgers, reporting, and taxes                                                       |
| Finance               | Budgeting and forecasting       | A company's plans, budgets, and financial models                                                       |
| Finance               | Payments and billing            | Invoices, payment systems, and vendor or customer billing                                              |
| Legal                 | Contracts                       | Agreements, terms, and their review                                                                    |
| Legal                 | Compliance and privacy          | Regulations, policies, and data-handling obligations                                                   |
| People and HR         | Hiring                          | Job descriptions, interviews, and candidates                                                           |
| People and HR         | Management                      | Feedback, performance, and running a team                                                              |
| Sales and marketing   | Sales                           | Prospects, outreach, and deals                                                                         |
| Sales and marketing   | Marketing content               | Posts, announcements, and promotional writing                                                          |
| Sales and marketing   | Customer support                | Answering and resolving customer issues                                                                |
| Operations            | Project coordination            | Schedules, status, meetings, and keeping work on track                                                 |
| Operations            | IT and administration           | Work devices, accounts, internal tools, and office administration                                      |
| Personal              | Money and taxes                 | The user's own savings, investments, accounts, and tax filings                                         |
| Personal              | Health and wellbeing            | Medical questions, fitness, nutrition, and mental wellbeing                                            |
| Personal              | Travel and transport            | Trips, routes, and getting around                                                                      |
| Personal              | Legal and government matters    | The user's own legal questions, visas, immigration, and paperwork                                      |
| Personal              | Career and education            | The user's own job search, professional growth, courses, and study                                     |
| Personal              | Shopping                        | Choosing and buying products                                                                           |
| Personal              | Home and food                   | Housing, household, garden, cooking, and recipes                                                       |
| Personal              | Devices and home tech           | The user's own computer, phone, network, apps, and accounts, apart from AI assistants                  |
| Personal              | Games and entertainment         | Games, films, music, and books                                                                         |
| Personal              | Hobbies, sports and leisure     | Sports, pets, vehicles, crafts, and other pastimes                                                     |
| Personal              | Family and relationships        | Family, friends, and social situations                                                                 |
| Personal              | General knowledge               | Math, science, history, news, or other learning with no sign of a job                                  |
| Other                 | No topic                        | Greetings and tests with no subject                                                                    |
| Other                 | Unclear                         | The subject cannot be told from the conversation                                                       |

## Pull request mapping (experimental)

- Off by default, behind an experimental flag set during configuration, and labeled experimental in
  the output.
- Local files only, and no code searches for pull requests. The same model call that assigns the
  type of work and topic also returns every pull request mentioned in the conversation, with its
  role. The model's answer is used as returned, with no check against the text.
- The model reads the stripped conversation, so it does not see tool output. It learns of a pull
  request from the user's messages, the assistant's replies, and the tool commands.
- A pull request is always identified by its full URL (`https://github.com/owner/repo/pull/7`),
  never a bare number, in the model's answer and the report. The prompt gives the session's
  repository so the model can resolve a bare number; with no known repository it omits the pull
  request.
- Roles, one per pull request: Contributed to (wrote, changed, merged, or maintained the pull
  request), Reviewed (judged it or handled review comments), Mentioned only (no work on it). The
  first two attribute the conversation's cost to the pull request; Mentioned only attributes
  nothing.
- A conversation that worked on several pull requests splits its cost evenly among them, and the
  report marks split amounts as estimates.
