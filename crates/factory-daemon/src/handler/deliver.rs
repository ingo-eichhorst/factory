//! A [`factory_task::deliver::PromptWriter`] backed by
//! [`factory_adapter::Adapter::send`] — crate docs decision 1's "supply one
//! backed by `Adapter::send`."
//!
//! [`factory_task::deliver::deliver`] journals the delivery attempt and
//! commits it *before* this is ever called (see that function's own module
//! docs) — this type's only job is turning "write this prompt to this
//! session" into one `Adapter::send` call once the journal row already
//! exists. It never reads or writes the store itself, and deliberately does
//! not hold a `&Store` at all: `deliver` itself already holds `&mut Store`
//! for the whole call, so a writer that also borrowed the store would be
//! holding a second, conflicting reference into it. The pane is resolved by
//! the caller (see `ops::task`) before `deliver` is ever called, while the
//! assignment that names the session is still fresh, and handed to this type
//! by value.

use factory_adapter::{Adapter, PaneId};
use factory_task::deliver::{PromptWriteError, PromptWriter};

pub(crate) struct AdapterPromptWriter<'a> {
    adapter: &'a (dyn Adapter + Send + Sync),
    task_id: uuid::Uuid,
    pane: Option<PaneId>,
}

impl<'a> AdapterPromptWriter<'a> {
    pub(crate) fn new(
        adapter: &'a (dyn Adapter + Send + Sync),
        task_id: uuid::Uuid,
        pane: Option<PaneId>,
    ) -> Self {
        Self {
            adapter,
            task_id,
            pane,
        }
    }
}

impl PromptWriter for AdapterPromptWriter<'_> {
    fn write_prompt(
        &mut self,
        _session_id: uuid::Uuid,
        prompt: &str,
    ) -> Result<(), PromptWriteError> {
        let Some(pane) = &self.pane else {
            // Nothing to write to, so nothing was written: this is a refusal,
            // not a failed delivery, and must not spend the task's one
            // authorised delivery.
            return Err(PromptWriteError::refused(
                "this session has no recorded pane to deliver to",
            ));
        };
        self.adapter.send(pane, self.task_id, prompt).map_err(|e| {
            // `SessionBusy` is the adapter's own precheck: it observed the
            // pane working and returned *before* submitting anything, which
            // is why it is a distinct variant from `SubmissionUnconfirmed`.
            // Reporting it as an ordinary failure would journal an attempt
            // that provably never reached the terminal, spending the task's
            // one authorised delivery and sending an operator to
            // `task resume` for a prompt nobody ever sent.
            //
            // Every other variant stays ambiguous on purpose, including
            // `SubmissionUnconfirmed` — there the keystrokes may well have
            // landed, and design §5 refuses to resend on a maybe.
            if matches!(e, factory_adapter::AdapterError::SessionBusy { .. }) {
                PromptWriteError::refused(e.to_string())
            } else {
                PromptWriteError::new(e.to_string())
            }
        })
    }
}
