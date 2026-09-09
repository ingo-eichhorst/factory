//! `Uuid <-> String` (de)serialization for every operation payload struct.
//!
//! Mirrors `envelope::uuid_str`/`envelope::opt_uuid_str` exactly, and for the
//! identical reason stated there: the `uuid` crate's `serde` feature is not
//! enabled in this workspace (checked in `Cargo.lock`), so `Uuid` has no
//! `Serialize`/`Deserialize` impl of its own. Those two modules are private
//! to `envelope.rs` and cover only the wire envelope's own fields
//! (`request_id`, `scope_id`); every operation payload field of type `Uuid`
//! or `Option<Uuid>` uses one of the two modules here instead.

/// For a required `Uuid` field: `#[serde(with = "crate::serde_uuid::required")]`.
///
/// Only a `deserialize` function — every payload struct in this crate derives
/// `Deserialize` only, never `Serialize`, so a `with` module pairing with it
/// needs no `serialize` half; unlike `envelope::uuid_str`, which pairs with
/// wire types that round-trip in both directions.
pub(crate) mod required {
    use serde::{Deserialize, Deserializer};
    use uuid::Uuid;

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Uuid::parse_str(&raw).map_err(serde::de::Error::custom)
    }
}

/// For an `Option<Uuid>` field: `#[serde(default, with = "crate::serde_uuid::optional")]`.
/// The `default` is still required alongside `with` — it is what makes a key
/// that is simply absent decode to `None` rather than a missing-field error,
/// mirroring `envelope::opt_uuid_str`'s own doc comment on the same point.
pub(crate) mod optional {
    use serde::{Deserialize, Deserializer};
    use uuid::Uuid;

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Uuid>, D::Error> {
        let raw: Option<String> = Option::deserialize(deserializer)?;
        raw.map(|s| Uuid::parse_str(&s).map_err(serde::de::Error::custom))
            .transpose()
    }
}
