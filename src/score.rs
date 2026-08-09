use crate::checks::{self, Metrics};
use crate::llm::Llm;
use crate::spec::Spec;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

pub const JUDGE_SYSTEM: &str = "You are a judge for ASO (App Store Optimization) listing copy. \
The author of the document is unknown; do not guess who wrote it. \
Listing keywords unnaturally, using unsubstantiated superlatives, or using flowery language \
unrelated to the app's actual features are grounds for deduction. Do not grade leniently, and \
back every score with a direct quote from the source document. \
Format, length, and banned words are handled by separate automated checks, so evaluate content quality only.";

/// Judging lens. Rotates every round.
/// (Repeating the same model correlates the error, so lens separation alone does not
///  produce an independent sample. Real independence comes from a panel of different models.)
pub const LENSES: &[&str] = &[
    "Weigh overall polish and adherence to the judging criteria in a balanced way.",
    "Pay special attention to whether the target keywords are worked in naturally, and whether it reads like keyword stuffing.",
    "Pay special attention to hook strength and conversion phrasing that would actually get someone to tap in store search results.",
    "Pay special attention to translation-ese or awkward localization phrasing.",
    "Look at readability and information density (can it be grasped at a glance).",
    "Look at the risk of violating review policy (trademark, exaggerated claims, pricing language).",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionScore {
    pub id: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub why_not_higher: String,
    pub score: f64, // 0-100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeResult {
    #[serde(default)]
    pub winning_conditions: Vec<String>,
    #[serde(default)]
    pub criteria: Vec<CriterionScore>,
    #[serde(default)]
    pub improvements: Vec<String>,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scored {
    pub label: String,
    /// 0-100 weighted sum
    pub total: f64,
    /// Aggregated score per criterion (0-100, trimmed mean)
    pub per_criterion: BTreeMap<String, f64>,
    /// All raw scores per criterion (per judge)
    pub raw: BTreeMap<String, Vec<f64>>,
    /// Max-min spread per criterion (verdict instability indicator)
    pub spread: BTreeMap<String, f64>,
    pub missing_fields: Vec<String>,
    /// Deterministic format check results
    pub format_issues: Vec<String>,
    pub metrics: Metrics,
    pub improvements: Vec<String>,
    pub comments: Vec<String>,
    pub rounds: usize,
    pub models: Vec<String>,
}

fn judge_schema(spec: &Spec) -> serde_json::Value {
    let ids: Vec<String> = spec.criteria.iter().map(|c| c.id.clone()).collect();
    // Field order = generation order. Having the judge write winning_conditions before
    // scoring reduces anchoring to the document (de-anchoring).
    json!({
        "type": "object",
        "properties": {
            "winning_conditions": {
                "type": "array",
                "minItems": 3,
                "items": {"type": "string"},
                "description": "Before reading the document, 3-6 conditions this listing must meet to capture both search visibility and conversion"
            },
            "criteria": {
                "type": "array",
                "minItems": ids.len(),
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "enum": ids},
                        "evidence": {"type": "string", "description": "Direct quote from the source document (a short one is fine; quoting the whole field is also allowed)"},
                        "why_not_higher": {"type": "string", "description": "Why not a higher score"},
                        "score": {"type": "integer", "minimum": 0, "maximum": 100}
                    },
                    "required": ["id", "evidence", "why_not_higher", "score"],
                    "additionalProperties": false
                }
            },
            "improvements": {
                "type": "array", "minItems": 3, "maxItems": 8,
                "items": {"type": "string", "description": "Immediately actionable revision instructions"}
            },
            "comment": {"type": "string"}
        },
        "required": ["winning_conditions", "criteria", "improvements", "comment"],
        "additionalProperties": false
    })
}

fn build_judge_prompt(spec: &Spec, doc: &str, lens: &str) -> String {
    format!(
        "# Task\nScore the submitted app store listing copy against the judging criteria.\n\n\
         ## Target store: {store}\n## App: {name}\n{ctx}\n\n\
         ## This judge's lens\n{lens}\n\n\
         ## Target keywords\n{kw}\n\n\
         ## Judging criteria (each item scored 0-100, integer)\n{rubric}\n\n\
         ## Score band criteria\n{bands}\n\n\
         ## Procedure\n\
         1. Before scoring the document, first write 3-6 'conditions this listing must meet' in winning_conditions.\n\
         2. Then score each criterion. For each item, directly quote the source document in evidence, and state in why_not_higher why a higher score was not given.\n\
         3. If you cannot find evidence to quote, that item cannot exceed 60 points.\n\
         4. Exceeding the character limit, missing required fields, and use of banned words are handled by separate automated checks, so do not factor them into scoring — evaluate copy quality only.\n\n\
         ## Document to score\n<document>\n{doc}\n</document>\n",
        store = spec.store.label(),
        name = spec.name,
        ctx = spec.context,
        lens = lens,
        kw = spec.keywords_prompt(),
        rubric = spec.rubric_prompt(),
        bands = spec.bands_prompt(),
        doc = doc
    )
}

/// Trimmed mean. If n>=4, drop one min and one max before averaging; otherwise a simple average.
/// (With many 0-100 integer samples, the median produces excessive ties and fails to detect small improvements)
fn trimmed_mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    if v.len() < 4 {
        return v.iter().sum::<f64>() / v.len() as f64;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let inner = &s[1..s.len() - 1];
    inner.iter().sum::<f64>() / inner.len() as f64
}

/// Scores a single document. Repeats `rounds` times, rotating model and lens.
/// `brief` is used for brief-vs-copy factual consistency checks (Some in gen/loop mode, None in score mode).
pub fn score_doc(
    judges: &[Llm],
    spec: &Spec,
    label: &str,
    doc: &str,
    rounds: usize,
    brief: Option<&str>,
) -> Result<Scored> {
    anyhow::ensure!(!judges.is_empty(), "No judge models available");
    let rounds = rounds.max(1);
    let schema = judge_schema(spec);
    let mut results: Vec<JudgeResult> = Vec::new();
    let mut models: Vec<String> = Vec::new();

    for i in 0..rounds {
        let llm = &judges[i % judges.len()];
        let lens = LENSES[i % LENSES.len()];
        let prompt = build_judge_prompt(spec, doc, lens);
        let v = llm
            .json(&prompt, Some(JUDGE_SYSTEM), &schema)
            .with_context(|| format!("Scoring failed ({label}, round {})", i + 1))?;
        let jr: JudgeResult = serde_json::from_value(v)
            .with_context(|| format!("Score result schema mismatch ({label})"))?;
        results.push(jr);
        models.push(llm.label());
    }

    let mut per_criterion: BTreeMap<String, f64> = BTreeMap::new();
    let mut raw: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut spread: BTreeMap<String, f64> = BTreeMap::new();
    for c in &spec.criteria {
        let vals: Vec<f64> = results
            .iter()
            .filter_map(|r| r.criteria.iter().find(|x| x.id == c.id))
            .map(|x| x.score.clamp(0.0, 100.0))
            .collect();
        let lo = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        spread.insert(c.id.clone(), if vals.is_empty() { 0.0 } else { hi - lo });
        per_criterion.insert(c.id.clone(), trimmed_mean(&vals));
        raw.insert(c.id.clone(), vals);
    }

    let wsum = spec.weight_sum();
    let total: f64 = spec
        .criteria
        .iter()
        .map(|c| per_criterion.get(&c.id).copied().unwrap_or(0.0) * (c.weight / wsum))
        .sum();

    let format_issues = checks::format_issues(spec, doc, brief);
    let missing = checks::missing_required(spec, doc);

    // If a judge never returns a particular criterion id (the schema can't prevent this),
    // that item is silently scored as 0, so we warn about it explicitly.
    let unscored: Vec<&str> = spec
        .criteria
        .iter()
        .filter(|c| raw.get(&c.id).map(|v| v.is_empty()).unwrap_or(true))
        .map(|c| c.id.as_str())
        .collect();

    let mut improvements: Vec<String> = format_issues.clone();
    if !unscored.is_empty() {
        improvements.push(format!(
            "[Scoring warning] Judges never returned a score for the following items, so they were scored as 0 (re-scoring recommended): {}",
            unscored.join(", ")
        ));
    }
    for r in &results {
        for imp in &r.improvements {
            let t = imp.trim().to_string();
            if !t.is_empty() && !improvements.contains(&t) {
                improvements.push(t);
            }
        }
    }

    Ok(Scored {
        label: label.to_string(),
        total: (total * 10.0).round() / 10.0,
        per_criterion,
        raw,
        spread,
        missing_fields: missing,
        format_issues,
        metrics: checks::metrics(spec, doc),
        improvements,
        comments: results.iter().map(|r| r.comment.clone()).collect(),
        rounds,
        models,
    })
}

/// Feedback for the regeneration prompt. Does not pass along the score itself (to discourage optimizing for score).
pub fn feedback_text(s: &Scored) -> String {
    let mut out = String::from("[Revisions that must be applied]\n");
    for i in &s.improvements {
        out.push_str(&format!("- {}\n", i));
    }
    if !s.comments.is_empty() {
        out.push_str("\n[Judges' overall comments]\n");
        for c in &s.comments {
            out.push_str(&format!("- {}\n", c));
        }
    }
    out
}

/// The 2 lowest-scoring items.
pub fn weak_points(spec: &Spec, s: &Scored) -> String {
    let mut v: Vec<(&str, f64)> = spec
        .criteria
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                s.per_criterion.get(&c.id).copied().unwrap_or(0.0),
            )
        })
        .collect();
    v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    v.iter()
        .take(2)
        .map(|(n, sc)| format!("- {} : {:.0}/100", n, sc))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimmed_mean_drops_outliers() {
        assert_eq!(trimmed_mean(&[70.0, 72.0, 74.0, 100.0]), 73.0);
        assert_eq!(trimmed_mean(&[80.0]), 80.0);
        assert!((trimmed_mean(&[70.0, 80.0]) - 75.0).abs() < 1e-9);
    }
}
