//! Canonical compatibility path for L5 Quality. This bridge preserves the
//! former synthetic-control API for callers; Quality evaluates L5 subjects.
pub use factory_assurance::quality::*;

pub fn applied_checks(tree: &QualityTree) -> Vec<crate::policy::Applied> {
    check_subjects(tree)
        .into_iter()
        .map(|s| crate::policy::Applied {
            control: s.control,
            title: s.title,
            kind: crate::policy::Kind::Standard,
            maps_to: s.maps_to,
            evidence: s.evidence,
            max_age: s.max_age,
            not_applicable: None,
            remediation: None,
            requires: Vec::new(),
        })
        .collect()
}
