# boethos-cli

Agent-first CLI for the [`boethos`](https://crates.io/crates/boethos) asset
catalogue. Install the binary with:

```sh
cargo install boethos-cli
```

The default database is `$BOETHOS_DB`, else
`$XDG_STATE_HOME/boethos/assets.redb`, else
`$HOME/.local/state/boethos/assets.redb`.

```sh
boethos index --root DIR [--root DIR ...] [--source LABEL] [--prune]
boethos list [--category C] [--sex S] [--family F] [--source S] [--tag T]
             [--name TEXT] [--offset N] [--limit N] [--json]
boethos get ID [--json]
boethos delete ID
boethos stats [--json]
boethos export PATH
```

Commands are non-interactive and strict about arguments. Read commands open an
existing database read-only; a missing database is a refusal, not an empty
catalogue. `--json` prints one document; text `list` prints `id`, `category`,
`sex`, `name` and `path`. `boethos --version` prints the binary version.

Exit `0` on success and `2` on a classified refusal. Errors name the path and
the failing capability.

License: GPL-3.0-only.
