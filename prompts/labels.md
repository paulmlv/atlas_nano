# Labels

The labels the labeling model chooses from, built into the binary. Each table row is one label: its
group or domain, its name, and the definition the model reads. Reports roll labels up by group and
domain. A changed row relabels every session.

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
