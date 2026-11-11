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

## Code

| Module       | Responsibility                                                  |
| ------------ | --------------------------------------------------------------- |
| `lib.rs`     | The library: the modules below                                  |
| `ingest/`    | Find and parse both clients' sessions; forks; each request once |
| `model.rs`   | What is kept of a session: its parts, turns, and model requests |
| `account.rs` | The period, each session's cost, and totals                     |
| `place.rs`   | A chat's project and repository, from the folder it started in  |
| `prices.rs`  | The built-in price table, `prices.toml`                         |

Parsers and accounting are pure functions tested on synthetic records. Money is `Decimal`.
