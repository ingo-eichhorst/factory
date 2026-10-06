//! L2 Environment's service (#193 phase 6, slice S6).
//!
//! It owns access to `L2State` (the sandbox provisioner and credential expiry
//! store) and serves the sandbox prerequisites: the provisioning pass, the
//! gateway, image, profile and provider reconcile, the dispatch-time
//! `sandbox_gate`, and the secret catalogue check. The method bodies live in
//! `provision.rs`.
//!
//! It reaches the rest of the daemon only through `Wiring<'a, L2>`: the
//! configuration snapshot, the binary path and the typed providers of L1 facts.
//! A downward or same-level provider is a compile error there.
//!
//! Page or service (D5), module by module:
//! - `provision.rs`: service.
//! - `secrets.rs`: the catalogue rows and metadata write are L2; the journal of
//!   changes lives in L4's task store and the `Request::Environment`/`SecretSet`
//!   answers are composed from both, so the Secrets tab is a **page** and stays
//!   `impl Engine` (its two `l4` reaches stay in the ratchet baseline).
//! - `dependencies.rs`: the report/VEX/document reads are provider wrappers
//!   (page); `attach_dependency` is a command a run's token authorises and
//!   journals in L4: composition at the router, not L2 state.
//! - `openshell.rs`, `service_observations.rs`: re-exports of the L2 crate.
//! - `prepare_sandbox` / `reconcile_openshell` (in `engine.rs`): they journal into
//!   L4 task entries and list L4 runs, so they move with the L3 -> L2 provision
//!   command (S8b), not here.
use crate::engine::Engine;
use crate::facts::Wiring;
use crate::state::L2State;
use factory_kernel::L2;

pub(crate) struct L2Service<'a> {
    pub(crate) state: &'a L2State,
    pub(crate) wiring: Wiring<'a, L2>,
}

impl Engine {
    pub(crate) fn l2_service(&self) -> L2Service<'_> {
        L2Service {
            state: &self.l2,
            wiring: Wiring::new(self),
        }
    }
}
