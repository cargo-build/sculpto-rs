# FIT-08 progress — 2026-11-12

**Status: In progress. This branch is a requested progress publication.**
The production fitting workflow has not passed the complete FIT-08 acceptance.
The engine-parity epic remains open. A successful reference experiment below
does not mark the integrated task Done.

## First qualifying static pants result

The pinned Apachii Nightshade pants now pass the measured static bounds through
an explicit source-preparation stage followed by the existing native static
fitter. The unchanged accepted target is the Bijin CBBE body plus its feet.

| Measurement | Result |
|---|---:|
| Actual body / feet triangle contacts | 0 / 0 |
| Inside groups | 0 |
| New self contacts / source-normal flips | 0 / 0 |
| Original fixed-frame silhouette IoU | 0.9014960274 |
| Required original silhouette baseline | 0.8883417222 |
| Maximum symmetric strain against retained original edges | 0.124071751 |
| Prepared / returned render topology | 370 vertices, 612 faces |
| Named openings / genus | waist + two cuffs / 0 |
| Retained original UV corners | exact |
| Native run wall time | 12.967 s |

The original source has 392 render vertices and 632 faces, with sealed lower
ends. Existing source-guided outward selection retains 611 original faces but
also omits one original crotch triangle, creating an unintended fourth opening.
The existing declared `incidental-disks` recovery restores that eligible
original triangle, with full boundary and source-corner provenance. Only the
20 source cuff-cap faces remain omitted. There is no contact-driven trimming,
new fill geometry, source vertex movement or altered original asset.

The prepared surface retains the natural flared hems and original front
silhouette (pre-fit IoU 1.0). It becomes the render input directly; reconstructing
the original inward caps through normal offsets had reintroduced foot contacts.
Prepared topology is preserved throughout fitting. Original geometry remains
the silhouette and retained-edge deformation reference.

The static reference run uses the existing native `cloth-fit` executable,
actual source/target guides, authored projection weights, body/self contact
constraints and `fit_weight=0` to avoid attraction to zero body stand-off.
It is a geometry-fitting experiment, not a new cloth simulator or an integrated
production backend. The paired returned avatar agrees with its target surface
within 4.99e-6 source units; acceptance independently checks the actual unchanged
body and feet, including the disclosed collision-proxy seam differences.

The reproducible host-gated checks are in
[source_disk_recovery.rs](crates/sculpto-mesh/tests/source_disk_recovery.rs):

```sh
SCULPTO_DISK_RECOVERY_FIXTURE=/path/to/preparation-evidence \
SCULPTO_DISK_NATIVE_FIXTURE=/path/to/qualified-native-evidence \
cargo test -p sculpto-mesh --test source_disk_recovery -- --nocapture
```

Private mod payloads and generated fixture data are not in this repository.
Without explicit fixture roots these tests print skips; a skip is not acceptance.
The frozen native executable is identified by SHA-256
`7031a255961465bf824804b66bb2ba13ff9ce1549004d03e6c4b3d2edbe51d71`.
This branch does not claim a portable native-backend installation.

### Matched geometry review

These are generated neutral review renders, with shared cameras and scale.
They show the complete original pants and the prepared/fitted result. They
do not replace the contact measurements or establish textured/posed acceptance.

| Original | Prepared and fitted |
|---|---|
| ![Original pants, front](assets/fit08/fitted612-front-source.png) | ![Fitted pants, front](assets/fit08/fitted612-front-control.png) |
| ![Original pants, side](assets/fit08/fitted612-side-source.png) | ![Fitted pants, side](assets/fit08/fitted612-side-control.png) |

## Integrated workflow and remaining work

[fit_acceptance.rs](crates/sculpto-engine/tests/fit_acceptance.rs) checks written
output independently of command reports: full solid references, source topology,
body contacts, inside counts, self contacts, orientation, fixed silhouette,
weights, locks and posed follow through GLB/Actor. It composes placed body and
feet through the existing named NIF reference API. The portable transformed
extra-part regression passes. The expanded configured run now includes male
feet too: male reference10630 vertices/20318 faces, female5825/10488.

The latest completed registered-command run fails Sentinel Armor001
(148 body contacts,74 inside), Armor001_1 (0 body +12 feet contacts,0 inside)
and original Nightshade pants (574 body +92 feet contacts,13 inside).
Belt and sleeves pass contact checks. Adding the previously omitted male feet
exposed the trousers'12 contacts; their earlier body-only result was incomplete.
Derived weight, mask, lock and pose controls pass on all five pieces.
Actual result:4 tests pass,1 configured geometry test fails,34.02s test time.
Build passes. This remains a failing workflow, distinct from the qualified
prepared-pants static experiment above.

Required next work:

- Integrate the qualified pants preparation/fitting route with explicit
  original/prepared/output identity, appearance and fitted pose verification.
- Resolve Sentinel without new folds or lost silhouette. Native and Rust
  diagnostic variants remain unqualified.
- Complete the robe's ordinary/Blender Cloth boundary acceptance.
- Run the complete configured acceptance and destination gates, then land
  both implementation and coordinator records. Only then mark FIT-08 Done.

Blender owns cloth simulation. Sculpto selects which parts/regions use Blender
Cloth and which use ordinary rigging; HDT/Bullet runtime work is outside this
scope. The operator requested the active goal be paused after FIT-08 is fully
verified, landed and marked Done.

## Negative evidence retained

The diagnostic tests on this branch preserve failed cases:
[fit recovery](crates/sculpto-mesh/tests/fit_recovery.rs),
[horizontal cuff preparation](crates/sculpto-mesh/tests/source_preparation.rs),
[native candidate composition](crates/sculpto-mesh/tests/native_candidate.rs)
and [unrecovered outward selection](crates/sculpto-mesh/tests/exterior_render.rs).
Some configured diagnostic cases deliberately fail their quality assertions.

A previous native frame-selection error is corrected: intermediate frame 1
was mistakenly treated as final. The last saved capped-source frame clears
contacts but fails the original silhouette frame. Applying original stand-offs
through the existing Rust FromShape API reintroduced 743 contacts and 42 new
self contacts. Neither result is accepted. The recovered open-surface result
above is a distinct, fully recorded source-construction route.

## UMA reference

The operator requested UMA intake before considering algorithm reuse.
[UMA upstream](https://github.com/umasteeringgroup/UMA) is present in the local
source collection, refreshed to master
`0d65997d319d8e08f478c540d171f03e5da1979a` and catalogued. Its clothing conformer,
binding data, mesh utility and tests are the bounded audit targets. No UMA
algorithm has been adopted or credited with closing FIT-08. Unreal is outside
this investigation.
