# boethos-cli

Agent-first CLI for the [`boethos`](../boethos) asset catalogue. Build it from
the workspace or use the `boethos-v0.1.0` GitHub release binary:

```sh
cargo build --release -p boethos-cli
# or
curl -LO https://github.com/cargo-build/sculpto-rs/releases/download/boethos-v0.1.0/boethos-0.1.0-x86_64-unknown-linux-musl
```

The default database is `$BOETHOS_DB`, else
`$XDG_STATE_HOME/boethos/assets.redb`, else
`$HOME/.local/state/boethos/assets.redb`.

```sh
boethos index --root DIR [--root DIR ...] [--file PATH ...] [--source LABEL] [--prune]
boethos list [--category C] [--sex S] [--family F] [--source S] [--tag T]
             [--name TEXT] [--offset N] [--limit N] [--json]
boethos get ID [--json]
boethos clips --glb PATH [--json]
boethos delete ID
boethos stats [--json]
boethos export PATH
```

`--file` indexes one hand-picked file with the same canonical-path identity the
recursive walk produces and requires `--source`. Commands are non-interactive
and strict about arguments. Read commands open an
existing database read-only; a missing database is a refusal, not an empty
catalogue. `clips --glb PATH` reads an actor document directly and needs no
database. `--json` prints one document; text `list` prints `id`, `category`,
`sex`, `name` and `path`. `boethos --version` prints the binary version.

Exit `0` on success and `2` on a classified refusal. Errors name the path and
the failing capability.

License: GPL-3.0-only.
