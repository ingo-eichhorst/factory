//! The Mac tab's answers: L1's `HostPower` (pmset reads and writes) composed with
//! the journal of who changed the mode, which lives in L4's task store. A page,
//! so it is `impl Engine` beside the entry point; `HostPower` itself (the runner,
//! the lock and the reading) stays in `host_power.rs`.
use super::*;

impl crate::engine::Engine {
    /// `Request::HostPowerMode`: read fresh, with the newest changes.
    pub(crate) async fn host_power_report(&self) -> PowerModeReport {
        let mut report = self.l1.host_power.read().await;
        report.changes = self.power_mode_changes().await;
        report
    }

    async fn power_mode_changes(&self) -> Vec<factory_core::protocol::PowerModeChange> {
        let entries = self
            .l4.store
            .entries(HOST_JOURNAL, CHANGES_SHOWN)
            .await
            .unwrap_or_default();
        entries
            .into_iter()
            .filter(|e| e.kind == POWER_MODE_CHANGED)
            .filter_map(|e| {
                let data = e.data.clone().unwrap_or_default();
                Some(factory_core::protocol::PowerModeChange {
                    at: e.at,
                    by: data
                        .get("by")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    from_ac: mode_of(&data, "from_ac"),
                    from_battery: mode_of(&data, "from_battery"),
                    to: mode_of(&data, "to")?,
                    message: e.message,
                })
            })
            .collect()
    }

    /// `Request::HostPowerModeSet`: set it, journal who changed it from
    /// what to what, and answer the report as it now reads.
    pub(crate) async fn set_host_power_mode(
        &self,
        caller: &crate::access::Caller,
        mode: PowerMode,
    ) -> Result<PowerModeReport> {
        let Changed {
            before,
            after,
            verification_error,
        } = self.l1.host_power.set(mode).await?;
        let asked = crate::operations::Asked::new(caller, None);
        let mut message = format!(
            "power mode: {} -> {mode} {}",
            modes_words(&before),
            asked.words()
        );
        if let Some(why) = &verification_error {
            message.push_str(&format!("; {why}"));
        }
        let entry = asked.entry(
            POWER_MODE_CHANGED,
            message,
            serde_json::json!({
                "from_ac": before.ac,
                "from_battery": before.battery,
                "to": mode,
                "after_ac": after.ac,
                "after_battery": after.battery,
                "confirmed": verification_error.is_none(),
                "verification_error": verification_error,
            }),
        );
        if let Err(e) = self.l4.store.append_entry(HOST_JOURNAL, &entry).await {
            return Err(FactoryError::Other(anyhow::anyhow!(
                "pmset accepted the change, but it could not be journaled: {e}; Refresh to inspect the host"
            )));
        }
        if let Some(why) = verification_error {
            return Err(FactoryError::Other(anyhow::anyhow!(why)));
        }
        tracing::info!("power mode set to {mode} {}", asked.words());
        Ok(self.host_power_report().await)
    }
}
