use crate::generate;
use crate::llm::Llm;
use crate::report;
use crate::score::{self, Scored};
use crate::spec::Spec;
use anyhow::Result;
use std::path::Path;

pub struct LoopOutcome {
    pub best_label: String,
    pub best_doc: String,
    pub best_score: Scored,
    pub first_doc: String,
    pub history: Vec<Scored>,
    pub stop_reason: String,
    /// Length inflation warning (volume increase relative to score)
    pub warnings: Vec<String>,
}

pub struct LoopCfg {
    pub target: f64,
    pub max_iter: usize,
    pub rounds: usize,
    /// If the improvement over the previous best score is below this value, it is considered stalled.
    pub min_delta: f64,
    /// If stalling continues for this many consecutive times, stop early.
    pub patience: usize,
}

/// Generate → score → regenerate with feedback loop.
/// The return value is not the last iteration but the best score (argmax) across all iterations.
pub fn run(gen_llm: &Llm, judges: &[Llm], spec: &Spec, idea: &str, out_dir: &Path, cfg: &LoopCfg, angle: &str) -> Result<LoopOutcome> {
    let mut doc = generate::generate(gen_llm, spec, idea, angle)?;
    let mut history: Vec<Scored> = Vec::new();
    let mut docs: Vec<String> = Vec::new();
    let mut best_i = 0usize;
    let mut stall = 0usize;
    let mut stop_reason = format!("Reached max iterations ({})", cfg.max_iter.max(1));

    for i in 0..cfg.max_iter.max(1) {
        let label = format!("iter{:02}", i + 1);
        std::fs::write(out_dir.join(format!("{}.md", label)), &doc)?;

        let s = score::score_doc(judges, spec, &label, &doc, cfg.rounds, Some(idea))?;
        report::append_jsonl(out_dir, &s)?;
        println!(
            "  [{}] {:.1}/100  ({} chars{})",
            label,
            s.total,
            s.metrics.total_chars,
            if s.format_issues.is_empty() { String::new() } else { format!(", {} format issue(s)", s.format_issues.len()) }
        );

        let prev_best = history.get(best_i).map(|b: &Scored| b.total);
        let improved = match prev_best {
            None => true,
            Some(b) => s.total > b,
        };
        history.push(s.clone());
        docs.push(doc.clone());
        if improved {
            let gain = s.total - prev_best.unwrap_or(f64::NEG_INFINITY);
            best_i = history.len() - 1;
            if prev_best.is_some() && gain < cfg.min_delta {
                stall += 1;
            } else {
                stall = 0;
            }
        } else {
            stall += 1;
        }

        if s.total >= cfg.target && s.format_issues.is_empty() {
            stop_reason = format!("Reached target score {:.0}", cfg.target);
            break;
        }
        if stall >= cfg.patience {
            stop_reason = format!("Improvement stalled ({} consecutive times below +{:.1} points)", cfg.patience, cfg.min_delta);
            break;
        }
        if i + 1 == cfg.max_iter.max(1) {
            break;
        }

        let fb = score::feedback_text(&history[history.len() - 1]);
        let weak = score::weak_points(spec, &history[history.len() - 1]);
        doc = generate::revise(gen_llm, spec, idea, &doc, &fb, &weak)?;
    }

    let best_score = history[best_i].clone();
    let best_doc = docs[best_i].clone();
    std::fs::write(out_dir.join("best.md"), &best_doc)?;

    // Length inflation canary: if the total character count increases excessively relative to the
    // score, suspect verbosity gaming.
    // (ASO fields have hard caps, so it won't run away as much as bizplan-loop, but the same trick
    //  could appear as filling multiple fields up to their max, so we keep the same canary.)
    let mut warnings = Vec::new();
    let first = &history[0];
    let d_score = best_score.total - first.total;
    let d_chars = best_score.metrics.total_chars as f64 - first.metrics.total_chars as f64;
    let growth = if first.metrics.total_chars > 0 { d_chars / first.metrics.total_chars as f64 } else { 0.0 };
    if growth > 0.25 && d_score < 5.0 {
        warnings.push(format!(
            "Length canary: total chars +{:.0}% but score only +{:.1} → may be padding rather than content improvement",
            growth * 100.0,
            d_score
        ));
    }
    if best_i + 1 < history.len() {
        warnings.push(format!(
            "Last iteration ({:.1} points) is not the best score → best.md is iter{:02}",
            history.last().map(|h| h.total).unwrap_or(0.0),
            best_i + 1
        ));
    }

    // Cross-iteration drift metric (applying Correlated Proxies, arXiv:2403.03185): if the score went
    // up but the document changed almost unrelated to the previous iteration, there's a risk that the
    // judge just rewrote the document to fit a "plausible-sounding" pattern, unrelated to actual
    // improvement — independent of the length canary (volume), this looks at the amount of change in
    // the content itself. Like the length canary, this is just a deterministic signal and, unlike a
    // held-out gate, it does not judge "what is correct."
    for i in 1..docs.len() {
        let d_score_round = history[i].total - history[i - 1].total;
        if d_score_round <= 0.0 {
            continue; // Not a target case if the score didn't go up, since this isn't a "score-only increase" case
        }
        let sim = jaccard_similarity(&docs[i - 1], &docs[i]);
        // Threshold 0.3: ASO fields are constrained to short lengths by the store's character cap, so
        // even a normal rewrite that just polishes some sentences tends to keep a good number of
        // common tokens (particles, key keywords, etc.). If the Jaccard similarity is below 0.3, we
        // treat it as more than just "polishing the wording" — effectively replaced with a different
        // document — and warn (no rigorous statistical basis — this is a conservatively chosen value).
        if sim < 0.3 {
            warnings.push(format!(
                "Drift warning: iter{:02}→iter{:02} score rose by {:+.1} but token Jaccard similarity is {:.2} \
                 (below 0.3) → content may have changed drastically while only the score went up; manual copy review recommended",
                i, i + 1, d_score_round, sim
            ));
        }
    }

    Ok(LoopOutcome { best_label: best_score.label.clone(), best_doc, first_doc: docs[0].clone(), best_score, history, stop_reason, warnings })
}

/// Jaccard similarity of whitespace-based token sets (intersection/union size ratio). Implemented
/// directly without adding an external crate — this is an approximation since it's a simple token
/// set comparison, not morphological analysis.
fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let ta: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let tb: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if ta.is_empty() && tb.is_empty() {
        return 1.0;
    }
    let inter = ta.intersection(&tb).count();
    let union = ta.union(&tb).count();
    if union == 0 {
        1.0
    } else {
        inter as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::jaccard_similarity;

    #[test]
    fn jaccard_similarity_identical_is_one() {
        assert_eq!(jaccard_similarity("budget expense-tracker app", "budget expense-tracker app"), 1.0);
    }

    #[test]
    fn jaccard_similarity_disjoint_is_zero() {
        assert_eq!(jaccard_similarity("budget expense-tracker", "totally different document"), 0.0);
    }

    #[test]
    fn jaccard_similarity_partial_overlap() {
        // {budget, expense-tracker, app} vs {budget, expense-tracker, full-edition} → intersection 2, union 4
        let sim = jaccard_similarity("budget expense-tracker app", "budget expense-tracker full-edition");
        assert!((sim - 0.5).abs() < 1e-9, "{sim}");
    }
}
