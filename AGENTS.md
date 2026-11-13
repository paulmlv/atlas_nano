# Working on Atlas Nano

Atlas Nano labels and accounts for all local Claude Code and Codex conversations to give insight
into AI spend.

## Development

Run from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Markdown is wrapped at 100 characters by [Prettier](.prettierrc.json), checked in CI:

```sh
npx prettier@3.9.9 --write $(git ls-files '*.md')
```

The prompts in [prompts/](prompts/) are left as written, since they are sent to the model as is.

[rust-toolchain.toml](rust-toolchain.toml) pins the Rust version; rustup installs it on first use.
Add dependencies with `cargo add` and commit `Cargo.toml` and `Cargo.lock` together. Tests use
synthetic sessions only, never real conversations.

To release, set `version` in `Cargo.toml`, run `cargo build` to update `Cargo.lock`, and merge both.
Then tag `main` with the same version: `git tag v0.1.0 && git push origin v0.1.0`. A tag with a
hyphen, such as `v0.2.0-rc1`, becomes a prerelease. The
[release workflow](.github/workflows/release.yml) tests and builds the binaries and publishes them
with [install.sh](install.sh), which downloads them.

Read the [command line](docs/CLI.md), [report](docs/REPORT.md), [labeling](docs/LABELING.md), and
[taxonomy](docs/TAXONOMY.md) pages before changing behavior they define, and update the page whose
fact changed. The prompt and the labels themselves are in [prompts/](prompts/). The
[labeling experiments](docs/LABELING_EXPERIMENTS.md) hold the measurements behind the labeling
design. The [next steps](docs/NEXT_STEPS.md) list what is left to test and decide.

## Code

| Module         | Responsibility                                                          |
| -------------- | ----------------------------------------------------------------------- |
| `account.rs`   | The period, each session's cost, and totals                             |
| `cli.rs`       | The commands and flags                                                  |
| `config.rs`    | The saved settings and the config, cache, and call folders              |
| `configure.rs` | `configure`: detection, questions, the test call, saving                |
| `ingest/`      | Find and parse both clients' sessions; forks; each request once         |
| `label.rs`     | The cache, the estimate, the parallel calls, and the chunk vote         |
| `labeler.rs`   | One labeling call through Claude Code or Codex                          |
| `lib.rs`       | The library the command is built on: the other modules                  |
| `main.rs`      | The `atlas-nano` command: parse the arguments and run a command         |
| `model.rs`     | What is kept of a session: its parts, turns, and model requests         |
| `paint.rs`     | Terminal colors, and when they are on                                   |
| `place.rs`     | A chat's project and repository, from the folder it started in          |
| `prices.rs`    | The built-in price table, `prices.toml`                                 |
| `prompt.rs`    | Fill the labeling prompt from `prompts/label.md`, built into the binary |
| `report.rs`    | The printed report and the CSV                                          |
| `run.rs`       | `run`: load, select the period, label, print                            |
| `taxonomy.rs`  | Read the label lists from `prompts/labels.md`, built into the binary    |
| `text.rs`      | Render a session's parts and split them into chunks                     |
| `tests/cli.rs` | The binary end to end, with a stand-in client                           |

Parsers and accounting are pure functions tested on synthetic records. Money is `Decimal`.

## Labeling calls

Every call to a labeling model uses the [required flags](docs/LABELING.md#required-flags). They keep
the client's own tools, skills, memory and instructions out of the call and stop it from saving a
session.
