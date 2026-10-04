//! Validated time spans shared by infrastructure and process configuration.
use serde::{Deserialize, Serialize};

/// A span of time as a person writes it in YAML: `30s`, `5m`, `2h`, `28d`.
/// Kept as the text it was written as, so a config written back reads the
/// same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Span(String);

impl Span {
    pub fn seconds(&self) -> u64 {
        parse_span(&self.0).expect("validated when parsed")
    }
}

impl TryFrom<String> for Span {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, String> {
        parse_span(&s)?;
        Ok(Span(s.trim().to_string()))
    }
}

impl From<Span> for String {
    fn from(s: Span) -> String {
        s.0
    }
}

/// `<n><unit>`, unit one of `s`, `m`, `h`, `d`; `n` a positive integer.
pub fn parse_span(text: &str) -> std::result::Result<u64, String> {
    let t = text.trim();
    let bad = || format!("{text:?} is not a span of time; write it as 30s, 5m, 2h or 28d");
    let unit = t.chars().last().ok_or_else(bad)?;
    let n: u64 = t[..t.len() - unit.len_utf8()].parse().map_err(|_| bad())?;
    let mult = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return Err(bad()),
    };
    if n == 0 {
        return Err(bad());
    }
    n.checked_mul(mult).ok_or_else(bad)
}
