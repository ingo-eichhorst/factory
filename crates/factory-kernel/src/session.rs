//! Opaque runtime session identity shared by commands and persisted records.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The identity of a live agent session, as the runtime adapter that created it
/// understands it. The daemon treats `handle` as opaque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub runtime: String,
    pub handle: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, String>,
}
