//! One module per decision-9 operation group, dispatched from
//! [`crate::handler`]. Every function here has the shape
//! `fn(&FactoryHandler, scope_id: Uuid, payload: Value) -> HandlerOutcome`,
//! parses its own payload (see `crate::errors::parse_payload`), and wires
//! straight into the domain crate that owns the rule — this module invents
//! none.

pub(crate) mod agent;
pub(crate) mod context;
pub(crate) mod schedule;
pub(crate) mod scope;
pub(crate) mod status;
pub(crate) mod task;
