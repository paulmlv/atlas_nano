# Atlas Nano

Atlas Nano labels and accounts for all local Claude Code and Codex conversations to give insight
into AI spend.

It reads the sessions both clients keep on this computer, prices every model request at API list
prices, and asks a small model, through your own Claude Code or Codex subscription, for each
session's type of work and topic. Nothing else leaves the computer.

## Install

On macOS (Apple silicon) or Linux (x86_64 or arm64):

```sh
curl -fsSL https://raw.githubusercontent.com/paulmlv/atlas_nano/main/install.sh | sh
```

The [script](install.sh) downloads the latest release's binary, checks its SHA-256, and puts it in
`~/.local/bin`. Set `ATLAS_NANO_INSTALL_DIR` for another folder, or `ATLAS_NANO_VERSION=v0.1.0`
for a given release. To remove it, delete the binary.

On any other system, build it from source with [Rust](https://rustup.rs):

```sh
cargo install --git https://github.com/paulmlv/atlas_nano --locked
```

## Quick start

```sh
atlas-nano configure
atlas-nano run --no-label   # accounting only, no model calls
atlas-nano run --confirm    # print the labeling estimate and ask first
```

## Documents

- [Command line](docs/CLI.md): the commands, their flags, and the configuration.
- [Report](docs/REPORT.md): the printed report, the CSV, and how each number is defined.
- [Labeling](docs/LABELING.md): what is sent to the labeling model and how the call is made.
- [Taxonomy](docs/TAXONOMY.md): how conversations are labeled, and the pull request mapping.
- [Prompt](prompts/label.md) and [labels](prompts/labels.md): what the labeling model is sent.
- [Next steps](docs/NEXT_STEPS.md): what is left to build, test, and decide.
- [Working on Atlas Nano](AGENTS.md): the contributor guide.

## License

[MIT](LICENSE)
