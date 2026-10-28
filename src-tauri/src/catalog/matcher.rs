//! Two-pass catalogue match for one embedded crop.
//!
//! Pass 1 scores only the item codes on the active BoM. A score at or above
//! the saved match threshold is `matched_bom`. Otherwise pass 2 scores the
//! whole catalogue. A global score at or above the threshold is `wrong_part`.
//! Anything weaker is `unrecognized`. An empty BoM list skips pass 1.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{CatalogCandidate, CatalogEntry};

const GLOBAL_CANDIDATES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchStatus {
    MatchedBom,
    WrongPart,
    Unrecognized,
}

/// One detection after the two catalogue passes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwoPassMatch {
    pub bounding_box: [i32; 4],
    pub status: MatchStatus,
    pub resolved_ic: Option<String>,
    pub description: Option<String>,
    pub part_type: Option<String>,
    pub score: f32,
    pub in_bom: bool,
    /// Top two global item codes. Omitted unless Settings > Training debug is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<CatalogCandidate>>,
}

/// Trimmed item codes. `None` and an empty list both skip pass 1.
pub(crate) fn bom_ic_set(ics: Option<Vec<String>>) -> Option<HashSet<String>> {
    let Some(ics) = ics else {
        return None;
    };
    let set: HashSet<String> = ics
        .into_iter()
        .map(|ic| ic.trim().to_string())
        .filter(|ic| !ic.is_empty())
        .collect();
    if set.is_empty() { None } else { Some(set) }
}

pub(crate) fn classify(
    entries: &[CatalogEntry],
    query: &[f32],
    bbox: [i32; 4],
    bom_ics: Option<&HashSet<String>>,
    threshold: f32,
    include_candidates: bool,
) -> TwoPassMatch {
    let ranked = rank(entries, query);
    let candidates =
        include_candidates.then(|| ranked.iter().take(GLOBAL_CANDIDATES).cloned().collect());
    let global = ranked.first();
    let bom_top = bom_ics.and_then(|allowed| {
        ranked
            .iter()
            .find(|candidate| allowed.contains(&candidate.ic))
    });
    if let Some(candidate) = bom_top.filter(|candidate| candidate.score >= threshold) {
        return named(bbox, MatchStatus::MatchedBom, candidate, true, candidates);
    }
    if let Some(candidate) = global.filter(|candidate| candidate.score >= threshold) {
        return named(bbox, MatchStatus::WrongPart, candidate, false, candidates);
    }
    let nearest = global.cloned();
    TwoPassMatch {
        bounding_box: bbox,
        status: MatchStatus::Unrecognized,
        resolved_ic: None,
        description: None,
        part_type: nearest
            .as_ref()
            .and_then(|candidate| blank_to_none(&candidate.part_type)),
        score: nearest
            .as_ref()
            .map(|candidate| candidate.score)
            .unwrap_or(0.0),
        in_bom: false,
        candidates,
    }
}

pub(crate) fn unrecognized_box(bbox: [i32; 4]) -> TwoPassMatch {
    TwoPassMatch {
        bounding_box: bbox,
        status: MatchStatus::Unrecognized,
        resolved_ic: None,
        description: None,
        part_type: None,
        score: 0.0,
        in_bom: false,
        candidates: None,
    }
}

fn named(
    bbox: [i32; 4],
    status: MatchStatus,
    candidate: &CatalogCandidate,
    in_bom: bool,
    candidates: Option<Vec<CatalogCandidate>>,
) -> TwoPassMatch {
    TwoPassMatch {
        bounding_box: bbox,
        status,
        resolved_ic: Some(candidate.ic.clone()),
        description: blank_to_none(&candidate.description),
        part_type: blank_to_none(&candidate.part_type),
        score: candidate.score,
        in_bom,
        candidates,
    }
}

fn blank_to_none(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Best score for each item code, highest first.
fn rank(entries: &[CatalogEntry], query: &[f32]) -> Vec<CatalogCandidate> {
    if entries.is_empty() || query.is_empty() {
        return Vec::new();
    }
    let mut best: HashMap<String, CatalogCandidate> = HashMap::new();
    for entry in entries {
        let Some(score) = dot(query, &entry.embedding) else {
            continue;
        };
        let replace = best
            .get(&entry.ic)
            .is_none_or(|current| score > current.score);
        if !replace {
            continue;
        }
        best.insert(
            entry.ic.clone(),
            CatalogCandidate {
                ic: entry.ic.clone(),
                description: entry.description.clone(),
                part_type: entry.part_type.clone(),
                score,
            },
        );
    }
    let mut ranked: Vec<CatalogCandidate> = best.into_values().collect();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.ic.cmp(&right.ic))
    });
    ranked
}

fn dot(query: &[f32], target: &[f32]) -> Option<f32> {
    if query.len() != target.len() || query.is_empty() {
        return None;
    }
    let mut score = 0.0f32;
    for (left, right) in query.iter().zip(target) {
        score += left * right;
    }
    if score.is_finite() { Some(score) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ic: &str, description: &str, part_type: &str, embedding: Vec<f32>) -> CatalogEntry {
        CatalogEntry {
            ic: ic.to_string(),
            description: description.to_string(),
            part_type: part_type.to_string(),
            embedding,
        }
    }

    fn unit(values: &[f32]) -> Vec<f32> {
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        values.iter().map(|value| value / norm).collect()
    }

    fn catalogue() -> Vec<CatalogEntry> {
        vec![
            entry("6PBB833", "Bracket", "bracket", unit(&[1.0, 0.0, 0.0])),
            entry("6PBB833", "Bracket", "bracket", unit(&[0.96, 0.28, 0.0])),
            entry("2ERGM22", "Hinge", "hinge", unit(&[0.0, 1.0, 0.0])),
            entry("4LRS006", "Rail", "rail", unit(&[0.0, 0.0, 1.0])),
        ]
    }

    fn bom(ics: &[&str]) -> HashSet<String> {
        ics.iter().map(|ic| (*ic).to_string()).collect()
    }

    #[test]
    fn a_bom_score_at_the_threshold_is_matched() {
        let entries = catalogue();
        let allowed = bom(&["6PBB833"]);
        let matched = classify(
            &entries,
            &unit(&[1.0, 0.0, 0.0]),
            [1, 2, 3, 4],
            Some(&allowed),
            0.70,
            false,
        );
        assert_eq!(matched.status, MatchStatus::MatchedBom);
        assert_eq!(matched.resolved_ic.as_deref(), Some("6PBB833"));
        assert_eq!(matched.description.as_deref(), Some("Bracket"));
        assert!(matched.in_bom);
        assert!((matched.score - 1.0).abs() < 1.0e-5);
        let value = serde_json::to_value(&matched).unwrap();
        assert!(value.get("candidates").is_none());
        assert_eq!(value["status"], "matched_bom");
    }

    #[test]
    fn a_weak_bom_score_and_a_strong_global_score_is_wrong_part() {
        let entries = catalogue();
        let allowed = bom(&["6PBB833"]);
        let matched = classify(
            &entries,
            &unit(&[0.0, 1.0, 0.0]),
            [0, 0, 8, 8],
            Some(&allowed),
            0.70,
            true,
        );
        assert_eq!(matched.status, MatchStatus::WrongPart);
        assert_eq!(matched.resolved_ic.as_deref(), Some("2ERGM22"));
        assert_eq!(matched.description.as_deref(), Some("Hinge"));
        assert!(!matched.in_bom);
        assert!((matched.score - 1.0).abs() < 1.0e-5);
        let candidates = matched.candidates.expect("debug candidates");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].ic, "2ERGM22");
        assert_eq!(candidates[1].ic, "6PBB833");
    }

    #[test]
    fn scores_below_the_threshold_are_unrecognized() {
        let entries = catalogue();
        let allowed = bom(&["6PBB833", "2ERGM22", "4LRS006"]);
        let query = unit(&[1.0, 1.0, 0.0]);
        let matched = classify(&entries, &query, [0, 0, 1, 1], Some(&allowed), 0.95, false);
        assert_eq!(matched.status, MatchStatus::Unrecognized);
        assert!(matched.resolved_ic.is_none());
        assert!(matched.description.is_none());
        assert_eq!(matched.part_type.as_deref(), Some("bracket"));
        assert!(matched.score < 0.95);
        assert!(!matched.in_bom);
    }

    #[test]
    fn an_empty_bom_list_uses_only_the_global_catalogue() {
        let entries = catalogue();
        let empty = bom_ic_set(Some(vec!["  ".to_string()]));
        assert!(empty.is_none());
        let matched = classify(
            &entries,
            &unit(&[0.0, 0.0, 1.0]),
            [4, 4, 9, 9],
            empty.as_ref(),
            0.70,
            false,
        );
        assert_eq!(matched.status, MatchStatus::WrongPart);
        assert_eq!(matched.resolved_ic.as_deref(), Some("4LRS006"));
        assert!(!matched.in_bom);
    }

    #[test]
    fn the_best_crop_of_one_item_code_is_kept() {
        let entries = catalogue();
        let allowed = bom(&["6PBB833"]);
        let matched = classify(
            &entries,
            &unit(&[0.96, 0.28, 0.0]),
            [0, 0, 2, 2],
            Some(&allowed),
            0.70,
            false,
        );
        assert_eq!(matched.status, MatchStatus::MatchedBom);
        assert!(matched.score > 0.99, "{}", matched.score);
    }
}
