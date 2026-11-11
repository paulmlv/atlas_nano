# Atlas Nano

Atlas Nano labels and accounts for local Claude Code and Codex conversations from the command line.

It reads the sessions both clients keep on this computer, prices every model request at API list
prices, and asks a small model, through your own Claude Code or Codex subscription, for each
session's type of work and topic. Nothing else leaves the computer.

## Quick start

```sh
cargo build --release
./target/release/atlas-nano configure
./target/release/atlas-nano run --no-label   # accounting only, no model calls
./target/release/atlas-nano run --confirm    # print the labeling estimate and ask first
```

## Documents

- [Command line](docs/CLI.md): the commands, their flags, and the configuration.
- [Report](docs/REPORT.md): the printed report, the CSV, and how each number is defined.
- [Labeling](docs/LABELING.md): what is sent to the labeling model and how the call is made.
- [Taxonomy](docs/TAXONOMY.md): the labels, their definitions, and the labeling rules.
- [Next steps](docs/NEXT_STEPS.md): what is left to build, test, and decide.
- [Working on Atlas Nano](AGENTS.md): the contributor guide.

## License

[MIT](LICENSE)
