//! The `keyword` knowledge provider: ranks vault pages by the words and tags
//! of a query against the index `knowledge::index` already builds. Keeps no
//! state of its own -- every search re-walks the vault, exactly as the
//! Knowledge tab does -- so there is nothing to rebuild and `changed` has
//! nothing to do.
//!
//! It reads what the index carries and nothing more: titles, ids, tags,
//! `area`, links and backlinks. Never a page body, so it needs no file-reading
//! rules of its own. A word that appears only in a page's text is not found;
//! that is the next provider's job, not this one's.

use factory_core::adapter::{KnowledgeHit, KnowledgeProvider, KnowledgeQuery};
use factory_core::error::{FactoryError, Result};
use factory_core::knowledge::{self, Index, Page};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// Integer weights, so ranking never depends on float comparison. A tag is a
/// deliberate statement of what a page is about, a title term nearly so, a
/// path segment less; `area` and a link only ever add to a page that already
/// earned a place (see `rank`).
const TAG: u32 = 4;
const TITLE: u32 = 2;
const PATH: u32 = 1;
const AREA: u32 = 1;
const NEIGHBOUR: u32 = 1;

/// Words that match too much to mean anything. English and German, the two
/// languages this company's vault is written in. A term shorter than two
/// characters is dropped as well.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "how", "in", "is", "it", "of", "on", "or",
    "the", "this", "to", "was", "what", "with", "am", "auf", "aus", "bei", "das", "dem", "den", "der", "des",
    "die", "ein", "eine", "einen", "für", "im", "ist", "mit", "oder", "und", "von", "wie", "zu", "zum", "zur",
];

pub struct KeywordKnowledge;

#[async_trait::async_trait]
impl KnowledgeProvider for KeywordKnowledge {
    fn name(&self) -> &str {
        "keyword"
    }

    fn description(&self) -> String {
        "keyword knowledge provider: tags, titles and links of the vault's own index, no page text".into()
    }

    async fn search(&self, root: &Path, query: &KnowledgeQuery) -> Result<Vec<KnowledgeHit>> {
        let root = root.to_path_buf();
        let query = query.clone();
        tokio::task::spawn_blocking(move || rank(&knowledge::index(&root), &query))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge search: {e}")))
    }
}

/// The whole ranking, pure: the same index and query always give the same
/// hits in the same order.
///
/// A page is *matched* when it carries every tag in `query.tags` and at
/// least one query term hits one of its tags, title words or path segments
/// -- or, with no terms at all, when the tag filter alone admits it. Only a
/// matched page can gain from `area`. A page one link away from a matched
/// page (either direction) gains `NEIGHBOUR` per such page and is a hit even
/// if nothing of its own matched, as long as it passes the tag filter too.
pub fn rank(index: &Index, query: &KnowledgeQuery) -> Vec<KnowledgeHit> {
    if !index.present || query.limit == 0 {
        return Vec::new();
    }
    let terms = terms(&query.text);
    let required: BTreeSet<String> = query.tags.iter().map(|t| normalize_tag(t)).filter(|t| !t.is_empty()).collect();
    if terms.is_empty() && required.is_empty() {
        return Vec::new();
    }
    let area = query.scope.as_deref().map(last_segment).map(str::to_lowercase);

    struct Scored<'a> {
        page: &'a Page,
        score: u32,
        tags: BTreeSet<&'a str>,
        title: BTreeSet<String>,
        path: BTreeSet<String>,
        area: bool,
        via: BTreeSet<&'a str>,
    }

    let admitted = |page: &Page| required.iter().all(|t| page.tags.iter().any(|p| p == t));

    let mut scored: BTreeMap<&str, Scored> = BTreeMap::new();
    for page in &index.pages {
        if !admitted(page) {
            continue;
        }
        let mut s = Scored {
            page,
            score: 0,
            tags: BTreeSet::new(),
            title: BTreeSet::new(),
            path: BTreeSet::new(),
            area: false,
            via: BTreeSet::new(),
        };
        let title_words: BTreeSet<String> = words(&page.title).collect();
        let path_words: BTreeSet<String> = words(&page.id.replace('/', " ")).collect();
        for term in &terms {
            for tag in &page.tags {
                if tag == term || last_segment(tag) == term {
                    s.tags.insert(tag.as_str());
                }
            }
            if title_words.contains(term) {
                s.title.insert(term.clone());
            } else if path_words.contains(term) {
                // A title usually repeats the file name; counting both would
                // double every page whose title was never written down.
                s.path.insert(term.clone());
            }
        }
        if terms.is_empty() {
            // The tag filter alone is the question, and this page answers
            // it. With terms as well, the filter only filters: the terms
            // decide what matched.
            s.tags.extend(page.tags.iter().filter(|t| required.contains(*t)).map(String::as_str));
        }
        s.score = TAG * s.tags.len() as u32 + TITLE * s.title.len() as u32 + PATH * s.path.len() as u32;
        if s.score == 0 {
            continue;
        }
        if let (Some(want), Some(have)) = (&area, &page.area) {
            if have.to_lowercase() == *want {
                s.area = true;
                s.score += AREA;
            }
        }
        scored.insert(page.id.as_str(), s);
    }

    // One hop, both directions, from the pages matched on their own account.
    let by_id: HashMap<&str, &Page> = index.pages.iter().map(|p| (p.id.as_str(), p)).collect();
    let matched: Vec<&str> = scored.keys().copied().collect();
    for id in matched {
        let page = by_id[id];
        for neighbour in page.links.iter().chain(&page.backlinks) {
            let Some(other) = by_id.get(neighbour.as_str()) else { continue };
            if !admitted(other) {
                continue;
            }
            let entry = scored.entry(other.id.as_str()).or_insert_with(|| Scored {
                page: other,
                score: 0,
                tags: BTreeSet::new(),
                title: BTreeSet::new(),
                path: BTreeSet::new(),
                area: false,
                via: BTreeSet::new(),
            });
            if entry.via.insert(id) {
                entry.score += NEIGHBOUR;
            }
        }
    }

    let mut hits: Vec<Scored> = scored.into_values().collect();
    hits.sort_by_key(|s| (Reverse(s.score), s.page.id.clone()));
    hits.truncate(query.limit);
    hits.into_iter()
        .map(|s| {
            let mut why = Vec::new();
            let tags: Vec<&str> = s.tags.iter().copied().collect();
            if !tags.is_empty() {
                why.push(format!("tags: {}", tags.join(", ")));
            }
            if !s.title.is_empty() {
                why.push(format!("title: {}", join(&s.title)));
            }
            if !s.path.is_empty() {
                why.push(format!("path: {}", join(&s.path)));
            }
            if s.area {
                why.push(format!("area: {}", s.page.area.as_deref().unwrap_or_default()));
            }
            if !s.via.is_empty() {
                let via: Vec<&str> = s.via.iter().copied().collect();
                why.push(format!("linked with: {}", via.join(", ")));
            }
            KnowledgeHit {
                page: s.page.id.clone(),
                title: s.page.title.clone(),
                score: s.score as f32,
                why: why.join("; "),
            }
        })
        .collect()
}

/// The query's terms: lowercased, split the way a tag is (letters, digits,
/// `_`, `-`, `/` stay together), a leading `#` dropped so `#invoicing` is
/// the tag it looks like, stop words and single characters removed,
/// deduplicated in the order first written.
fn terms(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for raw in text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '#')) {
        let term = normalize_tag(raw);
        let term = term.trim_matches(|c| c == '-' || c == '/' || c == '_');
        if term.chars().count() < 2 || STOP_WORDS.contains(&term) {
            continue;
        }
        if seen.insert(term.to_string()) {
            out.push(term.to_string());
        }
    }
    out
}

fn normalize_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('#').to_lowercase()
}

/// The words of a title or path, for exact term matching.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

fn last_segment(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str, title: &str, tags: &[&str], links: &[&str]) -> Page {
        Page {
            id: id.into(),
            title: title.into(),
            frontmatter: true,
            area: None,
            status: None,
            updated: None,
            sources: 1,
            links: links.iter().map(|s| s.to_string()).collect(),
            gaps: Vec::new(),
            backlinks: Vec::new(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            documents: Vec::new(),
        }
    }

    /// An index from pages, with backlinks filled in from their links the
    /// way `knowledge::index` would.
    fn index(mut pages: Vec<Page>) -> Index {
        let links: Vec<(String, String)> = pages
            .iter()
            .flat_map(|p| p.links.iter().map(move |l| (p.id.clone(), l.clone())))
            .collect();
        for (from, to) in links {
            if let Some(target) = pages.iter_mut().find(|p| p.id == to) {
                target.backlinks.push(from);
                target.backlinks.sort();
            }
        }
        Index {
            root: "/vault".into(),
            present: true,
            legacy: None,
            pages,
            tags: Vec::new(),
            documents: Vec::new(),
            gaps: Vec::new(),
            findings: Vec::new(),
        }
    }

    fn query(text: &str) -> KnowledgeQuery {
        KnowledgeQuery { text: text.into(), limit: 10, ..Default::default() }
    }

    fn ids(hits: &[KnowledgeHit]) -> Vec<&str> {
        hits.iter().map(|h| h.page.as_str()).collect()
    }

    #[test]
    fn a_tag_outranks_a_title_which_outranks_a_neighbour() {
        let idx = index(vec![
            page("billing/runbook", "Invoicing runbook", &[], &[]),
            page("clients/acme", "Acme", &["invoicing"], &["clients/hub"]),
            page("clients/hub", "Clients", &[], &[]),
            page("unrelated", "Unrelated", &["travel"], &[]),
        ]);
        let hits = rank(&idx, &query("invoicing"));
        assert_eq!(ids(&hits), ["clients/acme", "billing/runbook", "clients/hub"]);
        assert_eq!(hits[0].why, "tags: invoicing");
        assert_eq!(hits[1].why, "title: invoicing");
        assert_eq!(hits[2].why, "linked with: clients/acme");
    }

    #[test]
    fn equal_scores_are_ordered_by_page_id_every_time() {
        let idx = index(vec![
            page("zeta", "Zeta", &["ops"], &[]),
            page("alpha", "Alpha", &["ops"], &[]),
            page("mid", "Mid", &["ops"], &[]),
        ]);
        let first = rank(&idx, &query("ops"));
        assert_eq!(ids(&first), ["alpha", "mid", "zeta"]);
        for _ in 0..5 {
            assert_eq!(rank(&idx, &query("ops")), first);
        }
    }

    #[test]
    fn the_limit_cuts_after_ranking_not_before() {
        let idx = index(vec![
            page("a", "A", &[], &[]),
            page("b", "B", &["deploy"], &[]),
            page("c", "Deploy notes", &[], &[]),
        ]);
        let mut q = query("deploy");
        q.limit = 1;
        assert_eq!(ids(&rank(&idx, &q)), ["b"]);
        q.limit = 0;
        assert!(rank(&idx, &q).is_empty());
    }

    #[test]
    fn a_missing_vault_or_an_empty_query_is_no_hits_not_an_error() {
        let mut idx = index(vec![page("a", "Deploy", &["deploy"], &[])]);
        assert!(rank(&idx, &query("")).is_empty());
        assert!(rank(&idx, &query("the and of")).is_empty(), "stop words alone ask nothing");
        idx.present = false;
        assert!(rank(&idx, &query("deploy")).is_empty());
    }

    #[test]
    fn a_nested_tag_matches_its_last_segment_and_a_hash_is_ignored() {
        let idx = index(vec![page("acme", "Acme", &["clients/acme"], &[])]);
        assert_eq!(ids(&rank(&idx, &query("acme"))), ["acme"]);
        assert_eq!(ids(&rank(&idx, &query("#clients/acme"))), ["acme"]);
        assert_eq!(rank(&idx, &query("acme"))[0].score, (TAG + TITLE) as f32);
    }

    #[test]
    fn a_path_segment_counts_only_when_the_title_did_not() {
        let idx = index(vec![
            page("clients/acme", "Acme", &[], &[]),
            page("clients/beta", "Beta clients", &[], &[]),
        ]);
        let hits = rank(&idx, &query("clients"));
        assert_eq!(ids(&hits), ["clients/beta", "clients/acme"]);
        assert_eq!(hits[0].score, TITLE as f32);
        assert_eq!(hits[1].score, PATH as f32);
        assert_eq!(hits[1].why, "path: clients");
    }

    #[test]
    fn required_tags_filter_matches_and_neighbours_alike() {
        let idx = index(vec![
            page("a", "Deploy", &["ops"], &["b", "c"]),
            page("b", "B", &["ops"], &[]),
            page("c", "C", &[], &[]),
            page("d", "Deploy too", &[], &[]),
        ]);
        let mut q = query("deploy");
        q.tags = vec!["#OPS".into()];
        let hits = rank(&idx, &q);
        assert_eq!(ids(&hits), ["a", "b"], "c and d lack the tag, even though one is linked and one matches");
        assert_eq!(hits[1].why, "linked with: a", "b is here as a's neighbour, not for carrying the filter tag");
    }

    #[test]
    fn tags_alone_are_a_whole_question() {
        let idx = index(vec![
            page("a", "A", &["ops"], &[]),
            page("b", "B", &["ops", "billing"], &[]),
            page("c", "C", &["billing"], &[]),
        ]);
        let q = KnowledgeQuery { tags: vec!["ops".into()], limit: 10, ..Default::default() };
        assert_eq!(ids(&rank(&idx, &q)), ["a", "b"]);
    }

    #[test]
    fn area_only_lifts_a_page_that_already_matched() {
        let mut ours = page("ours", "Release checklist", &[], &[]);
        ours.area = Some("Factory".into());
        let mut idle = page("idle", "Idle", &[], &[]);
        idle.area = Some("factory".into());
        let idx = index(vec![page("theirs", "Release checklist", &[], &[]), ours, idle]);
        let mut q = query("release");
        q.scope = Some("projects/factory".into());
        let hits = rank(&idx, &q);
        assert_eq!(ids(&hits), ["ours", "theirs"]);
        assert_eq!(hits[0].why, "title: release; area: Factory");
    }

    #[test]
    fn a_page_linked_from_two_matches_gains_once_per_match() {
        let idx = index(vec![
            page("a", "Deploy", &[], &["hub"]),
            page("b", "Deploy again", &[], &["hub"]),
            page("hub", "Hub", &[], &[]),
        ]);
        let hits = rank(&idx, &query("deploy"));
        let hub = hits.iter().find(|h| h.page == "hub").unwrap();
        assert_eq!(hub.score, (2 * NEIGHBOUR) as f32);
        assert_eq!(hub.why, "linked with: a, b");
    }

    #[tokio::test]
    async fn searching_an_instance_with_no_vault_is_empty() {
        let root = std::env::temp_dir().join(format!("factory-keyword-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let hits = KeywordKnowledge.search(&root, &query("anything")).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn searching_a_real_vault_goes_through_the_index() {
        let root = std::env::temp_dir().join(format!("factory-keyword-test-{}", uuid::Uuid::new_v4()));
        let vault = knowledge::vault_root(&root);
        std::fs::create_dir_all(vault.join("clients")).unwrap();
        std::fs::write(
            vault.join("clients/acme.md"),
            "---\ntitle: Acme\ntags: [invoicing]\n---\nQuarterly, see [[terms]].\n",
        )
        .unwrap();
        std::fs::write(vault.join("terms.md"), "Net 30.\n").unwrap();
        let hits = KeywordKnowledge.search(&root, &query("invoicing")).await.unwrap();
        assert_eq!(ids(&hits), ["clients/acme", "terms"]);
        assert!(hits.iter().all(|h| !h.why.contains("Net 30")), "no page text in a hit");
    }
}
