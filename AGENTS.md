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

[rust-toolchain.toml](rust-toolchain.toml) pins the Rust version; rustup installs it on first use.
Add dependencies with `cargo add` and commit `Cargo.toml` and `Cargo.lock` together. Tests use
synthetic sessions only, never real conversations.

To release, set `version` in `Cargo.toml`, merge it, then push a matching tag:
`git tag v0.1.0 && git push origin v0.1.0`. The [release workflow](.github/workflows/release.yml)
builds the binaries that [install.sh](install.sh) downloads.

Read the [command line](docs/CLI.md), [report](docs/REPORT.md), [labeling](docs/LABELING.md), and
[taxonomy](docs/TAXONOMY.md) pages before changing behavior they define, and update the page whose
fact changed. The prompt and the labels themselves are in [prompts/](prompts/). The
[next steps](docs/NEXT_STEPS.md) list what is not built yet.

## Code

| Module         | Responsibility                                                                |
| -------------- | ----------------------------------------------------------------------------- |
| `account.rs`   | The period, each session's cost, and totals                                   |
| `cli.rs`       | The commands and flags                                                        |
| `config.rs`    | The saved settings and the config and cache folders                           |
| `configure.rs` | `configure`: detection, questions, the test call, saving                      |
| `ingest/`      | Find and parse both clients' sessions; forks; each request once               |
| `label.rs`     | The cache, the estimate, the parallel calls, and the chunk vote               |
| `labeler.rs`   | One labeling call through Claude Code or Codex                                |
| `lib.rs`       | The library the command is built on: the other modules                        |
| `main.rs`      | The `atlas-nano` command: parse the arguments and run a command               |
| `model.rs`     | What is kept of a session: its parts, turns, and model requests               |
| `paint.rs`     | Terminal colors, and when they are on                                         |
| `place.rs`     | A chat's project and repository, from the folder it started in                |
| `prices.rs`    | The built-in price table, `prices.toml`                                       |
| `prompt.rs`    | Fill the labeling prompt from `prompts/label.md`, built into the binary       |
| `report.rs`    | The printed report and the CSV                                                |
| `run.rs`       | `run`: load, select the period, label, print                                  |
| `taxonomy.rs`  | Read the label lists from `prompts/labels.md`, built into the binary          |
| `text.rs`      | Render a session's parts and split them into chunks                           |
| `tests/cli.rs` | The binary end to end, with a stand-in client                                 |

Parsers and accounting are pure functions tested on synthetic records. Money is `Decimal`.

## Labeling calls

Every call to a labeling model uses the [required flags](docs/LABELING.md#required-flags). They keep
the client's own tools, skills, memory and instructions out of the call and stop it from saving a
session.
