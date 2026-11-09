# Sculpto Garment add-on

The Blender surface of the sculpto engine: dress a character from a raw
garment asset, render audit screenshots, and convert NIFs. Per DECL0002 the
add-on is thin — every format read/write goes through the `sculpto` command
surface and every garment decision is one named entry in `rules.py`; this
add-on only builds Blender data.

## Layout

| File | Role |
|---|---|
| `sculpto_garment/__init__.py` | Operators, panel, registration (`bl_info` floor: Blender 4.2) |
| `sculpto_garment/rules.py` | The one decision table: garment classes, offsets, region and projection constants, each entry labelled with the proposed command id that will own it (F03 gap register, `RULE_GAPS`) |
| `sculpto_garment/pipeline.py` | Blender data construction: bmesh offset surface, placement baking, texture projection, weight wiring — consumes `rules.py` |
| `sculpto_garment/cli_bridge.py` | The one command driver: argv building, payload builders, envelope parsing, binary discovery, `run_cli` operator error surfacing, `CommandError`/`EnvelopeError` |
| `headless_run.py` | Headless operator run: import → bake → fit → export with count and byte assertions |

## CLI dependency

The add-on drives the `sculpto` binary built from this repository; without
it, dress and export report a `kind: message` operator error (the engine's
own classification, F06) and cancel.

1. Build (two jobs on the shared host):

   ```sh
   cargo build -j 2
   ```

2. Binary discovery (`cli_bridge.find_binary`): `$SCULPTO_BIN`, then
   `target/debug/sculpto` relative to the repository root, then `sculpto`
   on `PATH`.
3. Contract: `sculpto run --cmd <id> --input <json>`, one JSON envelope on
   stdout, exit codes 0 success / 2 invalid request / 1 refusal or
   environment failure, 300 s default timeout per command.
4. Command ids in use: `export.obj` (dress imports a raw NIF through the
   model), `export.nif` (operator conversion), and `bake` + `outfit.fit` in
   the headless script's preset and fit steps. The garment rules still
   computed in `rules.py` await proposed ids `garment.place`,
   `garment.region`, `garment.project`, `garment.weights` and
   `garment.offset` — none registered yet.

## Install

```sh
# from the repository root; <version> is e.g. 5.2 for Blender 5.2.3
mkdir -p ~/.config/blender/<version>/scripts/addons
ln -s "$PWD/tools/blender_addon/sculpto_garment" \
  ~/.config/blender/<version>/scripts/addons/sculpto_garment
```

Then enable *Sculpto Garment* under Edit > Preferences > Add-ons. A zip of
the `sculpto_garment` directory works through *Install from disk* too.

## Operator flows

### 1. Panel flow (View3D > Sidebar > Sculpto)

| Operator | id | Command ids | What it does |
|---|---|---|---|
| Dress character from raw asset | `sculpto_garment.dress` | `export.obj`; pending `garment.place` / `garment.region` / `garment.project` / `garment.weights` / `garment.offset` (proposed ids, `rules.py`) | poses the body at the clip's first frame, imports the raw asset (NIF through the command, glb/fbx/obj through Blender importers), scales and places it by class, rebuilds the covered body region as an offset surface, projects the raw's texture by ray-cast, transfers body weights |
| Render audit screenshots | `sculpto_garment.audit` | none — Blender render staging only | renders clip frames × views, plus garment-only frames with the actor's own outfit hidden |
| Export NIF through sculpto | `sculpto_garment.export_nif` | `export.nif` | converts a NIF to the `se`/`le` target, reports vertices/triangles/bytes |

Steps: set **Body**, pick the raw garment file, press **Dress character
from raw asset**; press **Render audit screenshots** into a target
directory; convert finished pieces with **Export NIF through sculpto**.

### 2. Headless flow

```sh
blender --background --factory-startup --python-exit-code 1 \
  --python tools/blender_addon/headless_run.py -- [--out DIR] [--bin PATH]
```

Runs the Tier-1 fixture through `export.obj`, `bake`, `outfit.fit` and
`export.nif`, asserts every imported vertex/triangle count against the
command envelope, byte-compares the command-surface bake against the plain
argv surface, and prints exactly one JSON summary line — exit 0 on success,
exit 1 with the failure summary otherwise.

## Tests

argv-level, no Blender, plain `python3 -m unittest`:

```sh
python3 tools/tests/test_sculpto_cli_bridge.py
python3 tools/tests/test_sculpto_rules.py
```

The first covers the bridge (argv shape, payload builders, envelope
parsing, `run_cli` error surfacing, fake-binary dispatch); the second covers
the rules table and asserts the F03 gap ids are still unregistered — the day
one appears, the test fails and the add-on copy behind it must be deleted.

Headless through Blender (F07; skips with a message under plain `python3`):

```sh
/run/current-system/sw/bin/blender --background --python \
  tools/blender_addon/tests/test_addon_headless.py
```

It imports the Tier-1 NIF through the add-on, converts it through the
export operator, and runs a corrupt NIF through the real `execute`,
asserting the CLI exit code and the reported `kind: message`.
