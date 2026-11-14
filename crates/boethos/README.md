# boethos

Typed, transactional artifact records and links over an embedded
[redb](https://crates.io/crates/redb) store.

`boethos` owns storage mechanics only: identities, schema versions, atomic
uniqueness, directed links and JSONL export. Consumers declare their record
kinds and validation. It has no character, garment, BodySlide or Blender
semantics.

```rust
use boethos::{Kind, Store};

struct Notes;

impl Kind for Notes {
    const NAME: &'static str = "note";
    const VERSION: u32 = 1;
    type Record = String;

    fn validate(_record: &Self::Record) -> Result<(), String> {
        Ok(())
    }
}

let store = Store::open("notes.redb")?;
store.upsert::<Notes>("hello", &"world".to_string())?;
assert_eq!(store.get::<Notes>("hello")?, Some("world".to_string()));
# Ok::<(), boethos::StoreError>(())
```

The store is local state. Export JSONL for a reviewable snapshot; do not commit
the database file.

License: GPL-3.0-only.
