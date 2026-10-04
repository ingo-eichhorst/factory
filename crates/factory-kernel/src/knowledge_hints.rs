//! Page references supplied with a dispatch; no page text, search or vault IO.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeHit {
    /// The vault page id: its path relative to the vault, without `.md`.
    pub page: String,
    pub title: String,
    /// Relative to this provider and this query only. The order of the hits
    /// is the answer; the number is there to show how far apart they are.
    pub score: f32,
    /// One line saying why the page matched, in words a person can check
    /// against the page.
    pub why: String,
}

/// The knowledge pages one run was handed at dispatch: the hits, and the
/// vault they are relative to, so a hit is `<vault>/<page>.md`. Carried on
/// the run's `TaskBinding` and recorded in its journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeHints {
    pub vault: String,
    pub hits: Vec<KnowledgeHit>,
}

impl KnowledgeHints {
    /// The file a hit names.
    pub fn path_of(&self, hit: &KnowledgeHit) -> String {
        format!("{}/{}.md", self.vault, hit.page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_hints_keep_their_wire_shape_and_nested_page_path() {
        let wire = serde_json::json!({"vault":"/tmp/vault", "hits":[{
            "page":"nested/page", "title":"Title", "score":0.5, "why":"matched title"
        }]});
        let hints: KnowledgeHints = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(hints.path_of(&hints.hits[0]), "/tmp/vault/nested/page.md");
        assert_eq!(serde_json::to_value(hints).unwrap(), wire);
    }
}
