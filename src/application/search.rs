//! Search across the inventory.
//!
//! Matching is a scored subsequence search written here instead of pulling in an
//! unmaintained fuzzy matcher: every query character must appear in order, with
//! bonuses for consecutive characters and for matches at a word start. A query
//! whose characters cannot all be found in order returns `None`, so a result is
//! never a guess.

use crate::domain::Component;

/// Highest score a single field can contribute, before bonuses.
const BASE: i32 = 1;
const CONSECUTIVE_BONUS: i32 = 4;
const WORD_START_BONUS: i32 = 6;
const EXACT_PREFIX_BONUS: i32 = 10;

/// Scores `query` against `haystack`, case-insensitively. `None` when not every
/// query character matched in order.
pub fn score(query: &str, haystack: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let query: Vec<char> = query.chars().flat_map(|character| character.to_lowercase()).collect();
    let haystack: Vec<char> = haystack.chars().flat_map(|character| character.to_lowercase()).collect();

    if query.len() > haystack.len() {
        return None;
    }

    let mut total = 0;
    let mut haystack_index = 0;
    let mut previous_match: Option<usize> = None;

    for wanted in &query {
        let mut found = None;
        while haystack_index < haystack.len() {
            if haystack[haystack_index] == *wanted {
                found = Some(haystack_index);
                haystack_index += 1;
                break;
            }
            haystack_index += 1;
        }
        let position = found?;
        total += BASE;
        if previous_match.is_some_and(|previous| previous + 1 == position) {
            total += CONSECUTIVE_BONUS;
        }
        let starts_word = position == 0
            || matches!(haystack[position - 1], ' ' | '-' | '_' | '.' | '/' | ':');
        if starts_word {
            total += WORD_START_BONUS;
        }
        previous_match = Some(position);
    }

    if haystack.starts_with(&query) {
        total += EXACT_PREFIX_BONUS;
    }
    Some(total)
}

/// Best score across every field a user may be looking at, plus the matched field
/// so the interface can say why something matched.
pub fn score_component(query: &str, component: &Component) -> Option<(i32, &'static str)> {
    if query.trim().is_empty() {
        return Some((0, ""));
    }

    let mut best: Option<(i32, &'static str)> = None;
    let mut consider = |field: &str, label: &'static str| {
        if let Some(found) = score(query, field)
            && best.is_none_or(|(current, _)| found > current)
        {
            best = Some((found, label));
        }
    };

    consider(&component.name, "name");
    consider(&component.id.native_id, "identifier");
    if let Some(description) = &component.description {
        consider(description, "description");
    }
    consider(&component.source.label(), "source");
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
        DetectionConfidence, DetectionMethod, PacmanOrigin,
    };

    fn component(name: &str, description: Option<&str>) -> Component {
        Component {
            id: ComponentId::new("pacman", name),
            name: name.to_string(),
            version: Some("1.0-1".to_string()),
            description: description.map(|value| value.to_string()),
            category: ComponentCategory::Package,
            source: ComponentSource::Pacman { origin: PacmanOrigin::Foreign },
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::command("pacman", vec!["-Q".to_string()]),
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: Vec::new(),
        }
    }

    #[test]
    fn an_empty_query_matches_everything_neutrally() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score_component("", &component("btop", None)), Some((0, "")));
        assert_eq!(score_component("   ", &component("btop", None)), Some((0, "")));
    }

    #[test]
    fn requires_every_character_in_order() {
        assert!(score("btp", "btop").is_some());
        assert_eq!(score("ptb", "btop"), None);
        assert_eq!(score("zzz", "btop"), None);
    }

    #[test]
    fn is_case_insensitive() {
        assert!(score("BTOP", "btop").is_some());
        assert!(score("btop", "BTOP").is_some());
    }

    #[test]
    fn a_prefix_scores_higher_than_a_scattered_match() {
        let prefix = score("btop", "btop").expect("prefix");
        let scattered = score("btop", "b-t-o-p-extra").expect("scattered");
        assert!(prefix > scattered, "prefix {prefix} should beat scattered {scattered}");
    }

    #[test]
    fn consecutive_characters_score_higher_than_gaps() {
        let solid = score("btop", "btop").expect("solid");
        let gapped = score("btop", "bxx-txx-o-p").expect("gapped");
        assert!(solid > gapped, "solid {solid} should beat gapped {gapped}");
    }

    #[test]
    fn matches_the_description_and_the_identifier_too() {
        let btop = component("btop", Some("A monitor of system resources"));
        assert_eq!(score_component("monitor", &btop).map(|(_, field)| field), Some("description"));
        assert_eq!(score_component("btop", &btop).map(|(_, field)| field), Some("name"));
        assert!(score_component("nothing-like-this", &btop).is_none());
    }

    #[test]
    fn reports_the_field_that_matched_best() {
        let btop = component("btop", Some("btop is a monitor"));
        let (score, field) = score_component("btop", &btop).expect("match");
        assert_eq!(field, "name", "the name wins over the description: score {score}");
    }

    #[test]
    fn a_query_longer_than_the_field_cannot_match() {
        assert_eq!(score("btop-extra", "btop"), None);
    }
}
