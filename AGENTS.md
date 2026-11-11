# Working on Atlas Nano

Atlas Nano labels and accounts for local Claude Code and Codex conversations from the command line.

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

Read the [labeling](docs/LABELING.md) and [taxonomy](docs/TAXONOMY.md) pages before changing
behavior they define, and update the page whose fact changed.

## Code

| Module        | Responsibility                                                                |
| ------------- | ----------------------------------------------------------------------------- |
| `lib.rs`      | The library: the modules below                                                |
| `config.rs`   | The saved settings and the config and cache folders                           |
| `ingest/`     | Find and parse both clients' sessions; forks; each request once               |
| `model.rs`    | What is kept of a session: its parts, turns, and model requests               |
| `account.rs`  | The period, each session's cost, and totals                                   |
| `place.rs`    | A chat's project and repository, from the folder it started in                |
| `prices.rs`   | The built-in price table, `prices.toml`                                       |
| `text.rs`     | Render a session's parts and split them into chunks                           |
| `taxonomy.rs` | Read the label lists and rules from `docs/TAXONOMY.md`, built into the binary |
| `labeler.rs`  | One labeling call through Claude Code or Codex                                |
| `label.rs`    | The cache, the estimate, the parallel calls, and the chunk vote               |

Parsers and accounting are pure functions tested on synthetic records. Money is `Decimal`.

## Labeling calls

Every call to a labeling model uses the [required flags](docs/LABELING.md#required-flags). They keep
the client's own tools, skills, memory and instructions out of the call and stop it from saving a
session.
