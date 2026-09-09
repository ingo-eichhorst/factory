//! Minting ids without `Uuid::new_v4()` or `Uuid::now_v7()`.
//!
//! `uuid` is pinned workspace-wide without the `v4` or `v7` feature (the
//! workspace manifest is centrally owned; see the crate root docs' decision
//! 5), so neither convenience constructor compiles here. `ids::new_id` builds
//! a real, spec-shaped UUIDv7 anyway, the way `factory_task::create::create`'s
//! own doc comment implies a caller must: through
//! [`uuid::Builder::from_unix_timestamp_millis`], which is not
//! feature-gated (checked directly against the `uuid` crate's own source —
//! only the `v1`/`v3`/`v4`/`v5`/`v6`/`v7`/`v8` *modules*, i.e. the convenience
//! constructors, sit behind `#[cfg(feature = ...)]`; the `Builder` methods
//! that do the same bit-twiddling by hand do not).
//!
//! The timestamp is real wall-clock time; the remaining "counter/random"
//! bytes the format calls for come from [`std::collections::hash_map::RandomState`]
//! (seeded from real OS entropy the first time it's used in this process —
//! documented behaviour, not an assumption) mixed with a per-call counter.
//! The counter is what this module actually relies on for uniqueness: two
//! ids minted in the same process can never collide, because the counter
//! never repeats, regardless of what the timestamp or the hash-seeded bits
//! do. Unpredictability across processes rests on `RandomState`; this is a
//! CLI minting a handful of ids per invocation, not a security token.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SEED: OnceLock<(u64, u64)> = OnceLock::new();
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Two OS-seeded 64-bit values, computed once per process.
fn seed() -> (u64, u64) {
    *SEED.get_or_init(|| {
        let random_state = RandomState::new();
        let mut first = random_state.build_hasher();
        first.write_u8(1);
        let mut second = random_state.build_hasher();
        second.write_u8(2);
        (first.finish(), second.finish())
    })
}

/// Ten bytes for [`uuid::Builder::from_unix_timestamp_millis`]'s
/// `counter_random_bytes`: unpredictable across processes (from `seed`),
/// guaranteed distinct within one (from `COUNTER`, whose low 16 bits are
/// folded in directly rather than only mixed, so uniqueness never depends on
/// the mixing itself being collision-free).
fn entropy_bytes() -> [u8; 10] {
    let (a, b) = seed();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = a ^ b.rotate_left(17) ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);

    let mut bytes = [0u8; 10];
    bytes[..8].copy_from_slice(&mixed.to_le_bytes());
    bytes[8..].copy_from_slice(&counter.to_le_bytes()[..2]);
    bytes
}

/// Mint a fresh, real UUIDv7: every `request_id` this crate sends, and every
/// `task_id`/`session_id` a command introduces (`task send`, `agent start`)
/// — decision 9's payload table requires both as caller-supplied.
#[must_use]
pub fn new_id() -> uuid::Uuid {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    uuid::Builder::from_unix_timestamp_millis(millis, &entropy_bytes()).into_uuid()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn minted_ids_are_shaped_as_uuid_v7() {
        let id = new_id();
        assert_eq!(id.get_version(), Some(uuid::Version::SortRand));
        assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
    }

    #[test]
    fn ten_thousand_ids_in_a_tight_loop_are_all_distinct() {
        // Forces same-millisecond collisions in the timestamp component, so
        // uniqueness here is proof the counter is doing its job, not an
        // artifact of the clock ticking between calls.
        let ids: HashSet<uuid::Uuid> = (0..10_000).map(|_| new_id()).collect();
        assert_eq!(
            ids.len(),
            10_000,
            "minted ids must never collide within a process"
        );
    }
}
