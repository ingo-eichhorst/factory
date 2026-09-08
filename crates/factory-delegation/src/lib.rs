//! The one version-1 delegation rule (design §6; backlog §8).
//!
//! > A human may target any scope. An agent may target a registered
//! > descendant or sibling scope, never an ancestor and never itself. A task
//! > carries its delegation chain, and a scope already in that chain cannot
//! > be targeted again.
//!
//! Skeleton only. The coordinator owns this file's module declarations, the
//! crate manifest, and the decisions recorded below, so that concurrent work
//! never collides in a centrally owned file; every other line belongs to the
//! slice's own agents.
//!
//! # This is cooperative policy, not security isolation
//!
//! Design §6 is explicit: every agent runs under one trusted macOS account
//! and can technically read files or invoke tools outside its scope. Nothing
//! in this crate is a security boundary. It stops an *honest* agent from
//! delegating somewhere it should not, and it stops two honest agents from
//! passing one task back and forth forever. It stops nothing else, and no
//! caller should be written as though it did.
//!
//! # Decisions the coordinator made before this crate was written
//!
//! **1. Sender identity is a session id, never a scope id.** An agent-facing
//! entry point takes the id of the *session that is asking* and resolves
//! `sessions.scope_id` from that row. It never accepts a scope id from a
//! caller claiming to be an agent, because a caller that may name its own
//! sender scope has no rule left to break. Design §6's "the sender scope is
//! supplied to the agent session by Factory" describes what the agent is
//! *told*; it is not a request for a new column. `sessions.scope_id` already
//! exists and is written only by `factory_session`.
//!
//! **2. Station 8 adds no migration.** `task_delegation_chain` is part of the
//! version-1 base schema (`factory_store::schema`), nothing has ever written
//! a row to it, and migration 3's rebuild of `tasks` left its
//! `REFERENCES tasks (id)` resolving correctly — migration 3's own doc
//! comment says so. Appending a migration 4 for this slice would be a
//! forward-only, append-only change (ADR 0012 decision 2) made for no
//! reason. Do not write one.
//!
//! **3. Two root scopes are not siblings.** A sibling shares a parent; a
//! scope with `parent_id IS NULL` shares nothing, so `Unrelated` is the
//! answer for a pair of roots. This is the direction that refuses more,
//! which is the safe direction for a trust rule, and it agrees with what
//! SQL's three-valued logic already does with `a.parent_id = b.parent_id`
//! when both are NULL. It also costs this instance nothing: the registry
//! declares exactly one root (`business-factory` at `.`), and every other
//! scope is a descendant of it. Rust's `Option<Uuid> == Option<Uuid>` says
//! the opposite, so this must be written down rather than left to whichever
//! language the check happens to be expressed in.
//!
//! **4. The chain is `0`-based, and the target is always last.** Position `0`
//! is the scope the task started at. A chain read back and sorted by
//! `position` is exactly the order the task travelled, so a `Vec` index and
//! a `position` value are the same number.
//!
//! **5. Where a chain comes from.** A task queued by a *human* against target
//! `T` has the chain `[T]` — the task has passed through exactly one scope.
//! A task queued by an agent *session* whose scope is `S` inherits the chain
//! of the task that session is currently running and appends `T`. If that
//! session is running no task, the chain is `[S, T]`. Including the sender's
//! own scope costs nothing — targeting self or an ancestor is already
//! refused — and it is what design §2.4's "every scope the task has passed
//! through" says.
//!
//! **6. The Rust check is the rule's home; the index is the backstop.** A
//! target already in the chain must be refused by this crate with a typed
//! error, before any row is written. `task_delegation_chain`'s
//! `UNIQUE (task_id, scope_id)` stays as the database's own last word, so a
//! future caller that bypasses this crate still cannot record a looping
//! chain — but a rusqlite constraint error is not the error this crate
//! returns.

pub mod queue;
pub mod rule;
