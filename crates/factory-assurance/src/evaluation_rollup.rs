//! Canonical counting and worst-scope folding of L5 check results. Producer
//! classifications remain opaque; L6 decides which bucket a result counts in.
use crate::checks::{EvaluationResult, StatusKind};
use factory_kernel::ControlRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCounts {
    #[serde(default)]
    pub satisfied: usize,
    #[serde(default)]
    pub attested: usize,
    #[serde(default)]
    pub stale: usize,
    #[serde(default)]
    pub open: usize,
    #[serde(default)]
    pub not_applicable: usize,
}

impl StatusCounts {
    pub fn add(&mut self, kind: StatusKind) {
        match kind {
            StatusKind::Satisfied => self.satisfied += 1,
            StatusKind::Attested => self.attested += 1,
            StatusKind::Stale => self.stale += 1,
            StatusKind::Open => self.open += 1,
            StatusKind::NotApplicable => self.not_applicable += 1,
        }
    }
}

fn compliance_severity(kind: StatusKind) -> u8 {
    match kind {
        StatusKind::Open => 0,
        StatusKind::Stale => 1,
        StatusKind::Attested => 2,
        StatusKind::Satisfied => 3,
        StatusKind::NotApplicable => 4,
    }
}

/// One status per control, folding many scopes' own check evaluation output
/// into the single view a subtree-wide rollup needs: "a control counts
/// compliant only if it is compliant in every scope it applies to" (ADR
/// 0004). For each control, every scope where it is `not_applicable` is
/// ignored -- that scope has nothing to say about whether it is met -- and
/// the worst of whatever real statuses remain wins (`Open` beats `Stale`
/// beats `Attested` beats `Satisfied`). A control that is `not_applicable`
/// in every scope it appears in keeps that status; a control absent from
/// every scope in `per_scope` never appears in the result at all.
///
/// Pure: no scope tree, no store, no clock -- just what each scope's own
/// `evaluate` already produced. The caller is the
/// one that knows which scopes are in the subtree being asked about.
pub fn worst_across_scopes<K: Clone>(
    per_scope: &[Vec<EvaluationResult<K>>],
) -> Vec<EvaluationResult<K>> {
    let mut worst: BTreeMap<ControlRef, EvaluationResult<K>> = BTreeMap::new();
    let mut fallback_na: BTreeMap<ControlRef, EvaluationResult<K>> = BTreeMap::new();

    for statuses in per_scope {
        for status in statuses {
            if status.status.kind() == StatusKind::NotApplicable {
                fallback_na
                    .entry(status.control.clone())
                    .or_insert_with(|| status.clone());
                continue;
            }
            match worst.get(&status.control) {
                Some(current)
                    if compliance_severity(current.status.kind())
                        <= compliance_severity(status.status.kind()) =>
                {
                    // The status already kept is at least as bad; nothing to do.
                }
                _ => {
                    worst.insert(status.control.clone(), status.clone());
                }
            }
        }
    }

    // A control that never had a real status anywhere it appeared is
    // `not_applicable` everywhere -- keep exactly one of those entries.
    for (control, status) in fallback_na {
        worst.entry(control).or_insert(status);
    }

    worst.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::{EvidenceRef, Status};
    fn row(id: &str, kind: &str, status: Status) -> EvaluationResult<String> {
        EvaluationResult {
            control: ControlRef::new("test", id),
            title: kind.into(),
            kind: kind.into(),
            refs: vec![EvidenceRef::task(kind)],
            status,
        }
    }
    #[test]
    fn worst_scope_is_generic_sorted_and_keeps_the_winners_metadata_and_first_tie() {
        let input = vec![
            vec![
                row(
                    "b",
                    "first",
                    Status::Satisfied {
                        reasons: vec!["first".into()],
                    },
                ),
                row(
                    "a",
                    "na",
                    Status::NotApplicable {
                        reasons: vec!["n/a".into()],
                    },
                ),
            ],
            vec![
                row(
                    "b",
                    "worse",
                    Status::Stale {
                        reasons: vec!["old".into()],
                    },
                ),
                row(
                    "a",
                    "real",
                    Status::Satisfied {
                        reasons: vec!["met".into()],
                    },
                ),
            ],
            vec![
                row(
                    "b",
                    "tie",
                    Status::Stale {
                        reasons: vec!["tie".into()],
                    },
                ),
                row(
                    "c",
                    "only-na",
                    Status::NotApplicable {
                        reasons: vec!["n/a".into()],
                    },
                ),
            ],
        ];
        let folded = worst_across_scopes(&input);
        assert_eq!(
            folded
                .iter()
                .map(|row| row.control.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(folded[0], input[1][1]);
        assert_eq!(folded[1], input[1][0]);
        assert_eq!(folded[2], input[2][1]);
        assert!(worst_across_scopes::<String>(&[]).is_empty());
    }
    #[test]
    fn counts_keep_every_status_and_the_existing_wire_shape() {
        let mut counts = StatusCounts::default();
        for kind in [
            StatusKind::Satisfied,
            StatusKind::Attested,
            StatusKind::Stale,
            StatusKind::Open,
            StatusKind::NotApplicable,
        ] {
            counts.add(kind);
        }
        assert_eq!(
            serde_json::to_value(counts).unwrap(),
            serde_json::json!({
                "satisfied":1, "attested":1, "stale":1, "open":1, "not_applicable":1,
            })
        );
        assert_eq!(
            serde_json::from_str::<StatusCounts>("{}").unwrap(),
            StatusCounts::default()
        );
    }
}
