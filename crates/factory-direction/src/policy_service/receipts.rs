//! L6 live reporting clocks and policy receipt commands. Lower evidence is
//! pulled through typed fact capabilities; own receipts stay append-only.
use super::Service;
use crate::{policy, reporting_clock};
use chrono::Utc;
use factory_kernel::{
    resolve_scope, scope_subtree, Attestation, ClockMark, ConfirmedSecurityReport, ControlRef,
    ExploitedFinding, FactoryError, Facts, KnowledgeTags, Provide, Result, Withdrawal, L6,
};
use reporting_clock::{ClockDeadlineState, ReportingClock};
use std::collections::BTreeSet;

impl Service<'_> {
    /// Resolve the authored subtree, read the actual lower facts and own
    /// receipt history, then compute deadlines live with the canonical fold.
    pub async fn clock<E, R>(
        &self,
        scope: Option<&str>,
        findings_provider: &E,
        reports_provider: &R,
    ) -> Result<ReportingClock>
    where
        E: Provide<
            ExploitedFinding,
            Query = String,
            Value = Vec<ExploitedFinding>,
            Error = FactoryError,
        >,
        R: Provide<
            ConfirmedSecurityReport,
            Query = Option<String>,
            Value = Vec<ConfirmedSecurityReport>,
            Error = FactoryError,
        >,
    {
        let (asked, targets) = scope_subtree(&self.intent.config.scopes, scope)?;
        let mut names: BTreeSet<String> = targets.into_iter().map(|s| s.name.clone()).collect();
        if let Some(asked) = &asked {
            names.insert(asked.name.clone());
        }
        let facts = Facts::<L6>::new();
        let mut findings = Vec::new();
        for name in &names {
            findings.extend(
                facts
                    .get::<ExploitedFinding, _>(findings_provider, name)
                    .await?,
            );
        }
        let reports = facts
            .get::<ConfirmedSecurityReport, _>(reports_provider, &scope.map(str::to_string))
            .await?;
        let attestations = self.intent.receipts.all().await?;
        Ok(reporting_clock::compute(
            &findings,
            &reports,
            &attestations,
            Utc::now(),
        ))
    }

    /// Validate and record a policy attestation or clock receipt under its
    /// original control/scope rules. Caller naming is transport input only.
    #[allow(clippy::too_many_arguments)]
    pub async fn attest<K, E, R>(
        &self,
        by: &str,
        control: ControlRef,
        scope: String,
        evidence: String,
        note: Option<String>,
        expires_at: chrono::DateTime<Utc>,
        clock: Option<ClockMark>,
        corrective: Option<reporting_clock::CorrectiveMeasureMark>,
        knowledge: &K,
        findings: &E,
        reports: &R,
    ) -> Result<Attestation>
    where
        K: Provide<KnowledgeTags, Query = (), Value = KnowledgeTags, Error = FactoryError>,
        E: Provide<
            ExploitedFinding,
            Query = String,
            Value = Vec<ExploitedFinding>,
            Error = FactoryError,
        >,
        R: Provide<
            ConfirmedSecurityReport,
            Query = Option<String>,
            Value = Vec<ConfirmedSecurityReport>,
            Error = FactoryError,
        >,
    {
        if evidence.trim().is_empty() {
            return Err(FactoryError::BadRequest(
                "evidence must not be empty".into(),
            ));
        }
        let now = Utc::now();
        if expires_at <= now {
            return Err(FactoryError::BadRequest(format!(
                "expires_at {expires_at} must be in the future"
            )));
        }
        if clock.is_some() && corrective.is_some() {
            return Err(FactoryError::BadRequest(
                "record a submission or a corrective measure, not both".into(),
            ));
        }
        if corrective.as_ref().is_some_and(|m| m.available_at > now) {
            return Err(FactoryError::BadRequest(
                "available_at must not be in the future".into(),
            ));
        }
        if clock.is_some() || corrective.is_some() {
            let art_14 = reporting_clock::art_14();
            if control != art_14 {
                return Err(FactoryError::BadRequest(format!(
                    "a reporting-clock submission may only be recorded against {art_14}, not {control}"
                )));
            }
        }

        // Canonicalizes the name the same way every other scoped write does,
        // and refuses one that names no scope at all.
        let scope = resolve_scope(&self.intent.config.scopes, &scope)?
            .name
            .clone();
        let (catalogues, _findings, _tags) = self.intent.catalogues_with_tags(knowledge).await?;
        let chain = self.intent.config.chain(&scope);
        let (applied, _findings) = policy::applicable(&catalogues, &chain);
        let found = applied.iter().find(|a| a.control == control).ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "{control} does not apply at {scope:?}, or is not a control any loaded catalogue defines"
            ))
        })?;
        if let Some(na) = &found.not_applicable {
            return Err(FactoryError::BadRequest(format!(
                "{control} is marked not applicable at {:?}: {}",
                na.scope, na.rationale
            )));
        }

        if let Some(mark_item) = clock
            .as_ref()
            .map(|m| &m.item)
            .or_else(|| corrective.as_ref().map(|m| &m.item))
        {
            // The item's own scope must *equal* the canonical `scope` --
            // `policy_clock(Some(&scope))` rolls up the subtree the same way
            // `Request::Policy` does, so a descendant's item can appear in
            // it too; only an exact match may be attested here.
            let clock_now = self.clock(Some(&scope), findings, reports).await?;
            let item = clock_now
                .items
                .iter()
                .find(|i| &i.item == mark_item)
                .ok_or_else(|| {
                    FactoryError::BadRequest(format!(
                        "{} is not a reporting-clock item in {scope:?}'s subtree",
                        mark_item
                    ))
                })?;
            if item.scope != scope {
                return Err(FactoryError::BadRequest(format!(
                    "{} belongs to scope {:?}, not {scope:?} -- attest it there",
                    mark_item, item.scope
                )));
            }
            if let Some(state) = &item.excluded {
                return Err(FactoryError::BadRequest(format!(
                    "{} is excluded ({state}); there is nothing left to report",
                    mark_item
                )));
            }
            if let Some(mark) = &clock {
                let deadline = item
                    .deadlines
                    .iter()
                    .find(|d| d.deadline == mark.deadline)
                    .ok_or_else(|| FactoryError::BadRequest("record an evidenced corrective measure before submitting the final report".into()))?;
                if matches!(
                    deadline.state,
                    ClockDeadlineState::Met | ClockDeadlineState::Late
                ) {
                    return Err(FactoryError::BadRequest(format!(
                        "{} already has a live submission for its {} deadline",
                        mark.item, mark.deadline
                    )));
                }
            }
            if corrective.is_some() && item.corrective_measure.is_some() {
                return Err(FactoryError::BadRequest("this item already has a live corrective-measure record; withdraw it before correcting it".into()));
            }
        }

        let attestation = Attestation {
            id: uuid::Uuid::new_v4().to_string(),
            control,
            scope,
            evidence,
            note,
            attested_by: by.to_string(),
            attested_at: now,
            expires_at,
            withdrawn: None,
            clock,
            corrective,
        };
        self.intent
            .receipts
            .append_attestation(&attestation)
            .await?;
        Ok(attestation)
    }

    /// Withdraw a previously recorded attestation: `Request::PolicyWithdraw`.
    /// Appends a new row referencing `id` -- the store refuses a second
    /// withdrawal of the same attestation on its own, so this only refuses
    /// the friendlier way, before the write is even attempted.
    pub async fn withdraw(
        &self,
        by: &str,
        id: String,
        reason: Option<String>,
    ) -> Result<Attestation> {
        let existing = self
            .intent
            .receipts
            .get(&id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such attestation: {id:?}")))?;
        if existing.withdrawn.is_some() {
            return Err(FactoryError::BadRequest(format!(
                "attestation {id:?} is already withdrawn"
            )));
        }
        let withdrawal = Withdrawal {
            at: Utc::now(),
            by: by.to_string(),
            reason,
        };
        self.intent
            .receipts
            .append_withdrawal(&id, &existing.control, &existing.scope, &withdrawal)
            .await?;
        let mut withdrawn = existing;
        withdrawn.withdrawn = Some(withdrawal);
        Ok(withdrawn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{policy_intent, policy_store::PolicyStore};
    use factory_kernel::{FactProvider, IntakeSource, SourceKind, L2, L4, L5};
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Mutex,
        },
    };

    struct Fixture {
        root: PathBuf,
        store: PolicyStore,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("factory-clock-owner-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(policy::policies_dir(&root)).unwrap();
            std::fs::write(policy::policies_dir(&root).join("cra.yaml"), "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - {id: art-14, title: Reporting, evidence: [{check: attestation}]}\n  - {id: manual, title: Manual, evidence: [{check: attestation}]}\n").unwrap();
            let store = PolicyStore::open(&root.join("receipts.sqlite")).unwrap();
            Self { root, store }
        }
        fn service(&self) -> Service<'_> {
            let config = policy_intent::Configuration {
                scopes: [
                    ("parent", "company", "."),
                    ("child", "demo", "projects/demo"),
                ]
                .into_iter()
                .map(|(id, name, path)| policy_intent::Scope {
                    id: id.into(),
                    name: name.into(),
                    path: path.into(),
                    policies: Default::default(),
                })
                .collect(),
                root_policies: serde_yaml_ng::from_str("frameworks: [cra]").unwrap(),
                root_name: Some("company".into()),
                instance_name: "instance".into(),
            };
            Service::new(policy_intent::Service::new(
                self.root.clone(),
                config,
                &self.store,
            ))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
    #[derive(Default)]
    struct Tags {
        calls: AtomicUsize,
        fail: AtomicBool,
    }
    impl FactProvider for Tags {
        type Level = L5;
    }
    #[async_trait::async_trait]
    impl Provide<KnowledgeTags> for Tags {
        type Query = ();
        type Value = KnowledgeTags;
        type Error = FactoryError;
        async fn get(&self, _: &()) -> Result<KnowledgeTags> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err(FactoryError::adapter("knowledge", "unavailable"));
            }
            Ok(KnowledgeTags {
                tags: Default::default(),
            })
        }
    }
    #[derive(Default)]
    struct Findings {
        calls: Mutex<Vec<String>>,
        fail: AtomicBool,
        rows: Mutex<Vec<ExploitedFinding>>,
    }
    impl FactProvider for Findings {
        type Level = L2;
    }
    #[async_trait::async_trait]
    impl Provide<ExploitedFinding> for Findings {
        type Query = String;
        type Value = Vec<ExploitedFinding>;
        type Error = FactoryError;
        async fn get(&self, scope: &String) -> Result<Self::Value> {
            self.calls.lock().unwrap().push(scope.clone());
            if self.fail.load(Ordering::SeqCst) {
                return Err(FactoryError::adapter("findings", "unavailable"));
            }
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|row| row.scope == *scope)
                .cloned()
                .collect())
        }
    }
    #[derive(Default)]
    struct Reports {
        calls: Mutex<Vec<Option<String>>>,
        fail: AtomicBool,
        rows: Mutex<Vec<ConfirmedSecurityReport>>,
    }
    impl FactProvider for Reports {
        type Level = L4;
    }
    #[async_trait::async_trait]
    impl Provide<ConfirmedSecurityReport> for Reports {
        type Query = Option<String>;
        type Value = Vec<ConfirmedSecurityReport>;
        type Error = FactoryError;
        async fn get(&self, scope: &Option<String>) -> Result<Self::Value> {
            self.calls.lock().unwrap().push(scope.clone());
            if self.fail.load(Ordering::SeqCst) {
                return Err(FactoryError::adapter("reports", "unavailable"));
            }
            Ok(self.rows.lock().unwrap().clone())
        }
    }
    fn report(id: &str) -> ConfirmedSecurityReport {
        let now = Utc::now();
        ConfirmedSecurityReport {
            item: id.into(),
            scope: "demo".into(),
            awareness_at: now - chrono::Duration::hours(1),
            source: IntakeSource {
                kind: SourceKind::Ui,
                reference: None,
                provider: None,
                relayed_by: None,
                repository: None,
                number: None,
                external_id: None,
            },
            confirmed_by: "owner".into(),
            confirmed_at: now,
            parent: None,
        }
    }

    #[tokio::test]
    async fn clock_owns_sorted_subtree_reads_and_preserves_lower_then_receipt_failure_order() {
        let fixture = Fixture::new();
        let owner = fixture.service();
        let findings = Findings::default();
        let reports = Reports::default();
        assert_eq!(
            owner
                .clock(Some("missing"), &findings, &reports)
                .await
                .unwrap_err()
                .code(),
            "no_such_scope"
        );
        assert!(findings.calls.lock().unwrap().is_empty());
        reports.rows.lock().unwrap().push(report("security-1"));
        let first = owner
            .clock(Some("company"), &findings, &reports)
            .await
            .unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(*findings.calls.lock().unwrap(), ["company", "demo"]);
        assert_eq!(*reports.calls.lock().unwrap(), [Some("company".into())]);
        reports.rows.lock().unwrap().clear();
        assert!(owner
            .clock(Some("projects/demo"), &findings, &reports)
            .await
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            reports.calls.lock().unwrap().last(),
            Some(&Some("projects/demo".into()))
        );
        let conn = rusqlite::Connection::open(fixture.root.join("receipts.sqlite")).unwrap();
        conn.execute(
            "ALTER TABLE policy_attestations RENAME TO qa_hidden_receipts",
            [],
        )
        .unwrap();
        reports.fail.store(true, Ordering::SeqCst);
        findings.fail.store(true, Ordering::SeqCst);
        assert!(owner
            .clock(None, &findings, &reports)
            .await
            .unwrap_err()
            .to_string()
            .contains("findings: unavailable"));
        findings.fail.store(false, Ordering::SeqCst);
        assert!(owner
            .clock(None, &findings, &reports)
            .await
            .unwrap_err()
            .to_string()
            .contains("reports: unavailable"));
        reports.fail.store(false, Ordering::SeqCst);
        assert!(owner
            .clock(None, &findings, &reports)
            .await
            .unwrap_err()
            .to_string()
            .contains("policy_attestations"));
        conn.execute(
            "ALTER TABLE qa_hidden_receipts RENAME TO policy_attestations",
            [],
        )
        .unwrap();
        assert!(owner
            .clock(None, &findings, &reports)
            .await
            .unwrap()
            .items
            .is_empty());
    }

    #[tokio::test]
    async fn ordinary_attestations_keep_validation_priority_and_append_only_history_after_reopen() {
        let fixture = Fixture::new();
        let owner = fixture.service();
        let tags = Tags::default();
        let findings = Findings::default();
        let reports = Reports::default();
        let expiry = Utc::now() + chrono::Duration::days(3);
        tags.fail.store(true, Ordering::SeqCst);
        let error = owner
            .attest(
                "agent",
                "cra/manual".parse().unwrap(),
                "missing".into(),
                " ".into(),
                None,
                expiry,
                None,
                None,
                &tags,
                &findings,
                &reports,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("evidence must not be empty"));
        assert_eq!(tags.calls.load(Ordering::SeqCst), 0);
        let error = owner
            .attest(
                "agent",
                "cra/manual".parse().unwrap(),
                "missing".into(),
                "evidence".into(),
                None,
                expiry,
                None,
                None,
                &tags,
                &findings,
                &reports,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), "no_such_scope");
        assert_eq!(tags.calls.load(Ordering::SeqCst), 0);
        assert!(owner
            .attest(
                "agent",
                "cra/manual".parse().unwrap(),
                "demo".into(),
                "evidence".into(),
                None,
                expiry,
                None,
                None,
                &tags,
                &findings,
                &reports
            )
            .await
            .is_err());
        assert!(fixture.store.all().await.unwrap().is_empty());
        tags.fail.store(false, Ordering::SeqCst);
        let receipt = owner
            .attest(
                "reviewer",
                "cra/manual".parse().unwrap(),
                "projects/demo".into(),
                "evidence".into(),
                Some("note".into()),
                expiry,
                None,
                None,
                &tags,
                &findings,
                &reports,
            )
            .await
            .unwrap();
        assert_eq!(receipt.scope, "demo");
        assert_eq!(receipt.attested_by, "reviewer");
        assert!(findings.calls.lock().unwrap().is_empty());
        assert!(reports.calls.lock().unwrap().is_empty());
        let withdrawn = owner
            .withdraw("owner", receipt.id.clone(), Some("superseded".into()))
            .await
            .unwrap();
        assert_eq!(withdrawn.withdrawn.as_ref().unwrap().by, "owner");
        assert!(owner
            .withdraw("owner", receipt.id.clone(), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("already withdrawn"));
        assert!(owner
            .withdraw("owner", "missing".into(), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("no such attestation"));
        let reopened = PolicyStore::open(&fixture.root.join("receipts.sqlite")).unwrap();
        assert_eq!(reopened.get(&receipt.id).await.unwrap(), Some(withdrawn));
        let conn = rusqlite::Connection::open(fixture.root.join("receipts.sqlite")).unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT count(*) FROM policy_attestations", [], |row| row
                .get(0))
                .unwrap(),
            2,
            "withdrawal appends, never updates the original receipt"
        );
    }

    #[tokio::test]
    async fn clock_marks_are_exact_scope_live_and_withdrawal_removes_the_corrective_anchor() {
        use reporting_clock::{ClockDeadlineKind, ClockItemRef, CorrectiveMeasureMark};
        let fixture = Fixture::new();
        let owner = fixture.service();
        let tags = Tags::default();
        let findings = Findings::default();
        let reports = Reports::default();
        reports.rows.lock().unwrap().push(report("security-1"));
        let item = ClockItemRef::Report {
            item: "security-1".into(),
        };
        let expiry = Utc::now() + chrono::Duration::days(3);
        let available = Utc::now() - chrono::Duration::hours(1);
        let final_mark = ClockMark {
            item: item.clone(),
            deadline: ClockDeadlineKind::FinalReport,
        };
        let error = owner
            .attest(
                "owner",
                reporting_clock::art_14(),
                "company".into(),
                "notice".into(),
                None,
                expiry,
                Some(final_mark.clone()),
                None,
                &tags,
                &findings,
                &reports,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("belongs to scope"));
        let error = owner
            .attest(
                "owner",
                reporting_clock::art_14(),
                "demo".into(),
                "notice".into(),
                None,
                expiry,
                Some(final_mark.clone()),
                None,
                &tags,
                &findings,
                &reports,
            )
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("before submitting the final report"));
        let anchor = owner
            .attest(
                "owner",
                reporting_clock::art_14(),
                "demo".into(),
                "fix".into(),
                None,
                expiry,
                None,
                Some(CorrectiveMeasureMark {
                    item: item.clone(),
                    available_at: available,
                }),
                &tags,
                &findings,
                &reports,
            )
            .await
            .unwrap();
        let current = owner
            .clock(Some("demo"), &findings, &reports)
            .await
            .unwrap();
        assert_eq!(
            current.items[0].deadlines[2].due_at,
            available + chrono::Duration::days(14)
        );
        let submission = owner
            .attest(
                "owner",
                reporting_clock::art_14(),
                "demo".into(),
                "notice".into(),
                None,
                expiry,
                Some(final_mark.clone()),
                None,
                &tags,
                &findings,
                &reports,
            )
            .await
            .unwrap();
        assert!(owner
            .attest(
                "owner",
                reporting_clock::art_14(),
                "demo".into(),
                "duplicate".into(),
                None,
                expiry,
                Some(final_mark),
                None,
                &tags,
                &findings,
                &reports
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("already has a live submission"));
        let current = owner
            .clock(Some("demo"), &findings, &reports)
            .await
            .unwrap();
        assert_eq!(
            current.items[0].deadlines[2]
                .submission
                .as_ref()
                .unwrap()
                .attestation,
            submission.id
        );
        owner.withdraw("owner", anchor.id, None).await.unwrap();
        let current = owner
            .clock(Some("demo"), &findings, &reports)
            .await
            .unwrap();
        assert_eq!(current.items[0].deadlines.len(), 2);
        assert!(current.items[0].corrective_measure.is_none());
    }
}
