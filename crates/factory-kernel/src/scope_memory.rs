//! What a scope's agents keep between runs (`#339`): its facts, the owner's
//! profile and the skills it may improve, as one run is handed them at
//! dispatch. Plain text and paths only -- reading the files, the budget, the
//! secret check, proposals and curation all belong to L3, which produces this.
use serde::{Deserialize, Serialize};

/// Which of a scope's two memory files a write is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTarget {
    /// `memory.facts`: what earlier runs of this scope recorded.
    #[default]
    Facts,
    /// `memory.profile`: who the owner is and how they want to be asked.
    Profile,
}

impl MemoryTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Facts => "facts",
            Self::Profile => "profile",
        }
    }
}

impl std::fmt::Display for MemoryTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a run may do with a skill it is handed (`skills[].learn`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Learn {
    /// A better procedure is filed as a proposal; a person accepts it.
    #[default]
    Propose,
    /// A better procedure is written at once, and journaled on the run.
    Apply,
    /// The skill is read-only to every run.
    Off,
}

impl Learn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Propose => "propose",
            Self::Apply => "apply",
            Self::Off => "off",
        }
    }
}

impl std::fmt::Display for Learn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One memory file as a run is handed it: its entries, in file order, and
/// how much of its budget they use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFile {
    /// Absolute, on the daemon's host.
    pub path: String,
    pub max_chars: u32,
    /// Characters the entries use as written, separators included.
    pub chars: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<String>,
    /// The one owner profile every scope that says `shared: owner` reads.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
    /// Why the file could not be read. The run is handed no entries and
    /// still starts; memory is a help, never a reason not to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One skill a run is handed: where its procedure is, never its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillHandle {
    pub name: String,
    /// The skill's directory, absolute. `SKILL.md` is its procedure.
    pub path: String,
    pub learn: Learn,
    /// Set when curation archived the directory: the archive's id, which
    /// `factory skill restore <name>` brings back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<String>,
    /// Set when the directory is missing for any other reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SkillHandle {
    /// The procedure itself.
    pub fn skill_file(&self) -> String {
        format!("{}/SKILL.md", self.path.trim_end_matches('/'))
    }

    /// Whether the run can read it at all.
    pub fn present(&self) -> bool {
        self.archived.is_none() && self.error.is_none()
    }
}

/// A scope's memory and skills, read fresh from its files: the fact L3
/// produces (its `Fact` impl is with the others in `facts.rs`). Query: the
/// scope's name. A scope declaring none is the empty value, not an error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeMemory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<MemoryFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<MemoryFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillHandle>,
}

impl ScopeMemory {
    pub fn is_empty(&self) -> bool {
        self.facts.is_none() && self.profile.is_none() && self.skills.is_empty()
    }

    pub fn file(&self, target: MemoryTarget) -> Option<&MemoryFile> {
        match target {
            MemoryTarget::Facts => self.facts.as_ref(),
            MemoryTarget::Profile => self.profile.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_memory_keeps_its_wire_shape_and_omits_what_is_absent() {
        let wire = serde_json::json!({
            "facts": {"path": "/co/projects/radar/memory/MEMORY.md", "max_chars": 2200, "chars": 11,
                "entries": ["one", "two"]},
            "profile": {"path": "/co/memory/USER.md", "max_chars": 1400, "chars": 0, "shared": true},
            "skills": [{"name": "radar", "path": "/co/projects/radar/skill", "learn": "propose"},
                {"name": "old", "path": "/co/projects/radar/old", "learn": "off", "archived": "radar/old-1"}]
        });
        let memory: ScopeMemory = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(memory.skills[0].skill_file(), "/co/projects/radar/skill/SKILL.md");
        assert!(memory.skills[0].present());
        assert!(!memory.skills[1].present());
        assert_eq!(serde_json::to_value(&memory).unwrap(), wire);
        assert!(ScopeMemory::default().is_empty());
        assert_eq!(serde_json::to_value(ScopeMemory::default()).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn learn_and_target_spell_themselves_as_the_config_does() {
        assert_eq!(serde_json::to_value(Learn::Propose).unwrap(), "propose");
        assert_eq!(serde_json::from_value::<Learn>("apply".into()).unwrap(), Learn::Apply);
        assert_eq!(Learn::default(), Learn::Propose);
        assert_eq!(MemoryTarget::Profile.to_string(), "profile");
        assert_eq!(serde_json::to_value(MemoryTarget::Facts).unwrap(), "facts");
    }
}
