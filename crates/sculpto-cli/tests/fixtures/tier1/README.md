# Tier-1 command fixture

A minimal BodySlide home so the argv/command parity test needs no private
asset payload. The layout is the one the CLI resolves (`--home` defaults to the
project's grandparent):

```
SliderSets/Tier1.osp        slider set "Tier1 Body", one slider ("Widen")
SliderPresets/Tier1.xml     preset "Tier1 Default", Widen = big 50
ShapeData/Tier1/Body.nif    source shape "Tier1 Body", a 4-vertex quad
ShapeData/Tier1/Body.osd    morph block "Tier1Widen": vertex 1 +0.5, vertex 2 +1.0 on x
```

Provenance: the NIF is `sculpto_nif::write::se_shape::write_se_shape` output for
the `fixture_asset()` shape, and the `.osd` is the container layout documented in
`sculpto_core::shape_data` (there is no container writer yet). Both blobs are
pinned to that generator by `tier1_fixture_matches_its_generator` in
`../../run_matches_argv.rs`; regenerate after a deliberate format change:

```sh
FIXTURE_UPDATE=1 cargo test -p sculpto-cli --test run_matches_argv tier1_fixture
```

The fixture is deliberately not real BodySlide output: `bake` resolves the plan
and applies offsets to a source shape, and both surfaces must produce the same
bytes and the same counts from it. `outfit.fit` has no Tier-1 case yet, because
it needs a body and a garment shape in one NIF and the writer emits one shape
per file.
