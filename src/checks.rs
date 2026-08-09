//! Deterministic checks. No LLM, rule-based.
//! (Rationale: evaluation cost hierarchy — assertion/code rules → LLM judge, in that order, cheaper and more stable.
//!  Rewritten with the same design as bizplan-loop's checks.rs, adapted for the ASO domain.)
//!
//! ## Open-source porting source
//! `normalize_keyword` / `sanitize_keywords` / `normalize_text_for_match` were rewritten in Rust,
//! based on the following logic from
//! [semihcihan/App-Store-Optimization-CLI](https://github.com/semihcihan/App-Store-Optimization-CLI)
//! (MIT License) (porting the algorithm only, not copying code):
//! - `cli/domain/keywords/policy.ts` → `normalizeKeyword`(trim+lowercase), `sanitizeKeywords`(dedup via Set after normalization)
//! - `cli/shared/aso-keyword-utils.ts` → `normalizeTextForKeywordMatch`(after Unicode normalization, replaces non-letter/digit characters with whitespace, then cleans up whitespace)
//!
//! The original uses `.normalize("NFKC")` + the `\p{L}\p{N}\p{M}` Unicode regex, but this project
//! approximates this with `char::is_alphanumeric()` instead of adding the `unicode-normalization`
//! crate — this is not a full NFKC equivalent.

use crate::spec::{Section, Spec, Store};
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Metrics {
    pub total_chars: usize,
    pub field_chars: BTreeMap<String, usize>,
    pub keyword_coverage: usize,
    pub keyword_total: usize,
    pub matched_keywords: Vec<String>,
    /// Tokens that overlap across Apple dedup-target fields (title/subtitle/keywords, etc.)
    pub duplicate_keywords: Vec<String>,
    pub emoji_count: usize,
    pub banned_hits: Vec<String>,
}

/// Normalization for heading comparison: keep only alphanumerics and lowercase them (the LLM may
/// emit different casing like `## Title`/`## TITLE`, so match case-insensitively).
fn norm_head(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Splits into (heading, body) pairs based on headings starting with `#`. (Reuses bizplan-loop's structure)
pub fn split_sections(doc: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut cur_head = String::new();
    let mut cur_body = String::new();
    for line in doc.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            if !cur_head.is_empty() || !cur_body.trim().is_empty() {
                out.push((cur_head.clone(), cur_body.clone()));
            }
            cur_head = t.trim_start_matches('#').trim().to_string();
            cur_body.clear();
        } else {
            cur_body.push_str(line);
            cur_body.push('\n');
        }
    }
    if !cur_head.is_empty() || !cur_body.trim().is_empty() {
        out.push((cur_head, cur_body));
    }
    out
}

/// Maps spec field id -> body text (trimmed) from the document.
pub fn field_bodies(spec: &Spec, doc: &str) -> BTreeMap<String, String> {
    let secs = split_sections(doc);
    let mut map = BTreeMap::new();
    for s in &spec.sections {
        let want = norm_head(&s.title);
        // Match only on exact equality (after alphanumeric + case normalization). Previously this used a
        // bidirectional substring containment check, which had a bug where field names could collide —
        // e.g. "Subtitle" literally contains "Title" as a substring — causing matches on the wrong field.
        if let Some((_, body)) = secs
            .iter()
            .find(|(h, _)| !h.is_empty() && norm_head(h) == want)
        {
            map.insert(s.id.clone(), body.trim().to_string());
        }
    }
    map
}

// ---- Ported from MIT-licensed semihcihan/App-Store-Optimization-CLI ----

/// Keyword normalization: trim + lowercase.
/// Ported from: cli/domain/keywords/policy.ts::normalizeKeyword
pub fn normalize_keyword(k: &str) -> String {
    k.trim().to_lowercase()
}

/// Deduplicates after normalization (preserves order).
/// Ported from: cli/domain/keywords/policy.ts::sanitizeKeywords
pub fn sanitize_keywords(input: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for k in input {
        let n = normalize_keyword(k);
        if !n.is_empty() && seen.insert(n.clone()) {
            out.push(n);
        }
    }
    out
}

/// Text normalization for keyword matching: lowercase + replace non-alphanumeric/whitespace characters
/// with spaces + clean up whitespace.
/// Ported from: cli/shared/aso-keyword-utils.ts::normalizeTextForKeywordMatch
pub fn normalize_text_for_match(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    replaced
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- ASO domain checks ----

pub fn metrics(spec: &Spec, doc: &str) -> Metrics {
    let bodies = field_bodies(spec, doc);
    let mut field_chars = BTreeMap::new();
    let mut total_chars = 0usize;
    for s in &spec.sections {
        let n = bodies.get(&s.id).map(|b| b.chars().count()).unwrap_or(0);
        field_chars.insert(s.id.clone(), n);
        total_chars += n;
    }

    let full_text = bodies.values().cloned().collect::<Vec<_>>().join(" ");
    let normalized_doc = normalize_text_for_match(&full_text);

    let mut matched_keywords = Vec::new();
    for kw in &spec.target_keywords {
        let nk_match = normalize_text_for_match(&normalize_keyword(kw));
        if !nk_match.is_empty() && normalized_doc.contains(&nk_match) {
            matched_keywords.push(kw.clone());
        }
    }

    Metrics {
        total_chars,
        field_chars,
        keyword_coverage: matched_keywords.len(),
        keyword_total: spec.target_keywords.len(),
        matched_keywords,
        duplicate_keywords: duplicate_keywords_across_fields(spec, &bodies),
        emoji_count: full_text.chars().filter(|c| is_emoji(*c)).count(),
        banned_hits: banned_hits(spec, &full_text),
    }
}

/// List of words (tokens) that overlap across dedup-target fields (title/subtitle/keywords, etc.).
/// Apple automatically dedups title+subtitle+keywords when indexing, so placing the same keyword
/// in multiple fields just wastes character count.
fn duplicate_keywords_across_fields(spec: &Spec, bodies: &BTreeMap<String, String>) -> Vec<String> {
    let targets: Vec<&Section> = spec
        .sections
        .iter()
        .filter(|s| s.keyword_dedup_target)
        .collect();
    if targets.len() < 2 {
        return Vec::new();
    }
    let mut token_field_count: BTreeMap<String, usize> = BTreeMap::new();
    for s in &targets {
        let body = bodies.get(&s.id).cloned().unwrap_or_default();
        let normalized = normalize_text_for_match(&body);
        let mut seen_in_field = HashSet::new();
        for tok in normalized.split(' ') {
            if tok.chars().count() < 2 {
                continue; // exclude particle/single-character noise (char-based — if byte-based, a single Korean character wouldn't be filtered out)
            }
            if seen_in_field.insert(tok.to_string()) {
                *token_field_count.entry(tok.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut dups: Vec<String> = token_field_count
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(t, _)| t)
        .collect();
    dups.sort();
    dups
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x2190..=0x21FF | 0x2B00..=0x2BFF | 0xFE0F | 0x1F1E6..=0x1F1FF
    )
}

fn default_superlative_patterns() -> &'static [&'static str] {
    &[
        r"\bbest\b",
        r"\btop\s*1\b",
        r"#\s*1\b",
        r"\bno\.?\s*1\b",
        r"1\s*위",
        r"최고",
        r"최초",
        r"유일",
        r"업계\s*1위",
        r"가장\s*(좋은|빠른|정확한)",
    ]
}

fn default_price_patterns() -> &'static [&'static str] {
    &[
        r"\$\s*\d",
        r"\d+\s*%\s*(off|할인)",
        r"무료\s*체험",
        r"무료\s*다운로드",
        r"세일",
        r"특가",
        r"이벤트가",
    ]
}

fn compile_all(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|p| RegexBuilder::new(p).case_insensitive(true).build().ok())
        .collect()
}

/// Collects source-text snippets matching the spec's competitor/trademark patterns plus the default superlative/price patterns.
fn banned_hits(spec: &Spec, text: &str) -> Vec<String> {
    static SUPER_RE: OnceLock<Vec<Regex>> = OnceLock::new();
    static PRICE_RE: OnceLock<Vec<Regex>> = OnceLock::new();
    let super_res = SUPER_RE.get_or_init(|| compile_all(default_superlative_patterns()));
    let price_res = PRICE_RE.get_or_init(|| compile_all(default_price_patterns()));
    let user_terms: Vec<&str> = spec.banned_terms.iter().map(|s| s.as_str()).collect();
    let user_res = compile_all(&user_terms);

    let mut hits = Vec::new();
    for (label, res) in [
        ("superlative expression", super_res.as_slice()),
        ("price phrase", price_res.as_slice()),
        ("banned term (spec-defined)", user_res.as_slice()),
    ] {
        for re in res {
            if let Some(m) = re.find(text) {
                hits.push(format!("{}: \"{}\"", label, m.as_str()));
            }
        }
    }
    hits
}

/// A first-pass regex filter for quantitative/factual claims in the copy (applying Loki's
/// check-worthiness identification stage). Since this is pattern matching rather than an LLM
/// judgment, it may over-detect (e.g. when "official" is used in a different context) —
/// treat it as a reference signal for a human to make the final call.
fn default_claim_patterns() -> &'static [&'static str] {
    &[
        r"\d[\d,]*\s*(만|천|억)?\s*\+", // numeric claims like "10만+", "50,000+"
        r"\d+\s*위",                    // ranking claims like "1위"
        r"다운로드",
        r"수상",
        r"최초",
        r"업계\s*유일",
        r"공식",
    ]
}

/// A first-pass brief-vs-copy factual consistency check (applying FacTool/Loki).
/// Extracts quantitative/factual claim patterns from the copy and checks, via simple substring
/// containment, whether that expression also appears in the original `--brief` text (not an LLM
/// judgment). A claim absent from the brief may have been fabricated without basis, so it's left
/// as a warning.
/// If there is no `brief` (e.g. in `score` mode), there's nothing to compare against, so the check is skipped.
pub fn factual_claim_issues(doc: &str, brief: Option<&str>) -> Vec<String> {
    let brief = match brief {
        Some(b) if !b.trim().is_empty() => b,
        _ => return Vec::new(),
    };
    static CLAIM_RE: OnceLock<Vec<Regex>> = OnceLock::new();
    let res = CLAIM_RE.get_or_init(|| compile_all(default_claim_patterns()));

    let mut issues = Vec::new();
    let mut seen = HashSet::new();
    for re in res {
        for m in re.find_iter(doc) {
            let claim = m.as_str().trim().to_string();
            if claim.is_empty() || !seen.insert(claim.clone()) {
                continue;
            }
            if !brief.contains(&claim) {
                issues.push(format!(
                    "[factual consistency] claim not found in brief: \"{}\" → if the original brief has no basis for this, remove it from the copy or add supporting basis to the brief",
                    claim
                ));
            }
        }
    }
    issues
}

pub fn missing_required(spec: &Spec, doc: &str) -> Vec<String> {
    let bodies = field_bodies(spec, doc);
    spec.sections
        .iter()
        .filter(|s| s.required && bodies.get(&s.id).map(|b| b.is_empty()).unwrap_or(true))
        .map(|s| s.title.clone())
        .collect()
}

/// Deterministic findings related to format/length/keywords/banned terms.
/// If `brief` is present (gen/loop mode), also runs the brief-copy factual consistency check.
pub fn format_issues(spec: &Spec, doc: &str, brief: Option<&str>) -> Vec<String> {
    let mut issues: Vec<String> = Vec::new();
    let bodies = field_bodies(spec, doc);

    for m in missing_required(spec, doc) {
        issues.push(format!(
            "Required field '{}' missing → needs to be filled in",
            m
        ));
    }

    for s in &spec.sections {
        let n = bodies.get(&s.id).map(|b| b.chars().count()).unwrap_or(0);
        if n == 0 {
            continue; // missing case is already handled above
        }
        if n > s.max_chars {
            issues.push(format!(
                "'{}' exceeds character limit: {} chars (max {} chars) → may be truncated or rejected at store submission, needs shortening",
                s.title, n, s.max_chars
            ));
        } else if s.min_chars > 0 && n < s.min_chars {
            issues.push(format!(
                "'{}' below recommended character count: {} chars (recommended {}+ chars) → may waste exposure opportunity",
                s.title, n, s.min_chars
            ));
        }
        // Uncertain: there are reports that Apple's keywords field limit may actually be byte-based rather than character-based.
        if spec.store == Store::Apple && s.id == "keywords" && n as f64 > s.max_chars as f64 * 0.9 {
            issues.push(format!(
                "[uncertain] keywords field is at {} chars, close to the limit ({} chars) — it's unclear from the \
                 documentation whether Apple's keywords field is measured in characters or bytes, so verify directly \
                 in App Store Connect (be especially careful with multi-byte characters such as Korean)",
                n, s.max_chars
            ));
        }
    }

    if !spec.target_keywords.is_empty() {
        let deduped = sanitize_keywords(&spec.target_keywords);
        if deduped.len() < spec.target_keywords.len() {
            issues.push(format!(
                "spec's target_keywords contains duplicate entries ({} → {} after normalization) → recommend cleaning up in TOML",
                spec.target_keywords.len(),
                deduped.len()
            ));
        }
    }

    let m = metrics(spec, doc);
    if !m.duplicate_keywords.is_empty() {
        issues.push(format!(
            "{} keyword duplicate(s) across fields: {} → placing duplicates across auto-dedup target fields wastes character count",
            m.duplicate_keywords.len(),
            m.duplicate_keywords.join(", ")
        ));
    }
    if !spec.target_keywords.is_empty() && m.keyword_coverage < m.keyword_total {
        let missing: Vec<&str> = spec
            .target_keywords
            .iter()
            .filter(|k| !m.matched_keywords.contains(k))
            .map(|s| s.as_str())
            .collect();
        issues.push(format!(
            "target keyword coverage {}/{} → not covered: {}",
            m.keyword_coverage,
            m.keyword_total,
            missing.join(", ")
        ));
    }
    if m.emoji_count > spec.emoji_max {
        issues.push(format!(
            "{} emoji used (allowed {}) → excessive emoji may be perceived as spam, reduce usage",
            m.emoji_count, spec.emoji_max
        ));
    }
    for h in &m.banned_hits {
        issues.push(format!("banned expression detected — {} → needs to be replaced (trademark/false-advertising risk)", h));
    }

    issues.extend(factual_claim_issues(doc, brief));

    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_text_for_match_strips_punct_and_lowers() {
        assert_eq!(normalize_text_for_match("Hello, World!!"), "hello world");
        assert_eq!(normalize_text_for_match("가계부  #1  앱"), "가계부 1 앱");
    }

    #[test]
    fn sanitize_keywords_dedups_case_insensitively() {
        let v = vec![
            "Budget".to_string(),
            " budget ".to_string(),
            "Tracker".to_string(),
        ];
        assert_eq!(
            sanitize_keywords(&v),
            vec!["budget".to_string(), "tracker".to_string()]
        );
    }

    fn test_spec() -> Spec {
        use crate::spec::Criterion;
        Spec {
            name: "Test".into(),
            store: Store::Apple,
            context: String::new(),
            scoring_source: String::new(),
            target_keywords: vec!["가계부".into(), "지출관리".into()],
            banned_terms: vec!["뱅크샐러드".into()],
            emoji_max: 1,
            angles: vec![],
            bands: vec![],
            sections: vec![
                Section {
                    id: "title".into(),
                    title: "Title".into(),
                    guide: String::new(),
                    max_chars: 10,
                    min_chars: 0,
                    required: true,
                    keyword_dedup_target: true,
                },
                Section {
                    id: "subtitle".into(),
                    title: "Subtitle".into(),
                    guide: String::new(),
                    max_chars: 10,
                    min_chars: 0,
                    required: true,
                    keyword_dedup_target: true,
                },
            ],
            criteria: vec![Criterion {
                id: "x".into(),
                name: "x".into(),
                weight: 1.0,
                guide: String::new(),
            }],
        }
    }

    #[test]
    fn format_issues_flags_overlength_and_missing_and_coverage() {
        let spec = test_spec();
        // title exceeds 10 chars, subtitle missing, only 1 of 2 keywords covered, contains banned term 뱅크샐러드
        let doc = "## Title\n가계부 지출관리 완전정복판\n";
        let issues = format_issues(&spec, doc, None);
        assert!(issues
            .iter()
            .any(|i| i.contains("Title") && i.contains("exceeds")));
        assert!(issues
            .iter()
            .any(|i| i.contains("Subtitle") && i.contains("missing")));
    }

    #[test]
    fn format_issues_flags_duplicate_keyword() {
        let spec = test_spec();
        let doc = "## Title\n가계부\n## Subtitle\n가계부 앱\n";
        let issues = format_issues(&spec, doc, None);
        assert!(issues.iter().any(|i| i.contains("duplicate")));
    }

    #[test]
    fn format_issues_flags_banned_term() {
        let spec = test_spec();
        let doc = "## Title\n뱅크샐러드 대비 더 쉬운 가계부\n## Subtitle\n간편 가계부\n";
        let issues = format_issues(&spec, doc, None);
        assert!(
            issues
                .iter()
                .any(|i| i.contains("banned expression") && i.contains("뱅크샐러드")),
            "{issues:?}"
        );
    }

    #[test]
    fn norm_head_is_case_insensitive() {
        assert_eq!(norm_head("## Title"), norm_head("## TITLE"));
    }

    #[test]
    fn factual_claim_issues_flags_claim_absent_from_brief() {
        let doc = "## Title\n가계부\n## Subtitle\n누적 10만+ 다운로드 달성\n";
        let brief = "간편한 가계부 앱. 지출을 자동으로 분류해준다.";
        let issues = factual_claim_issues(doc, Some(brief));
        assert!(
            issues
                .iter()
                .any(|i| i.contains("factual consistency") && i.contains("10만")),
            "{issues:?}"
        );
    }

    #[test]
    fn factual_claim_issues_passes_when_claim_present_in_brief() {
        let doc = "## Title\n가계부\n## Subtitle\n누적 10만+ 다운로드 달성\n";
        let brief = "이 앱은 누적 10만+ 다운로드를 기록한 인기 가계부 앱이다.";
        let issues = factual_claim_issues(doc, Some(brief));
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn factual_claim_issues_skipped_without_brief() {
        let doc = "## Title\n가계부\n## Subtitle\n누적 10만+ 다운로드 달성\n";
        assert!(factual_claim_issues(doc, None).is_empty());
    }

    #[test]
    fn format_issues_boundary_exact_max_chars_ok_but_plus_one_fails() {
        let spec = test_spec(); // title max_chars = 10
        let ok_doc = "## Title\n1234567890\n## Subtitle\nab\n"; // exactly 10 chars
        let issues_ok = format_issues(&spec, ok_doc, None);
        assert!(
            !issues_ok
                .iter()
                .any(|i| i.contains("Title") && i.contains("exceeds")),
            "{issues_ok:?}"
        );

        let over_doc = "## Title\n12345678901\n## Subtitle\nab\n"; // 11 chars
        let issues_over = format_issues(&spec, over_doc, None);
        assert!(
            issues_over
                .iter()
                .any(|i| i.contains("Title") && i.contains("exceeds")),
            "{issues_over:?}"
        );
    }

    #[test]
    fn metrics_counts_keyword_coverage() {
        let spec = test_spec();
        let doc = "## Title\n가계부\n## Subtitle\n지출관리 완벽\n";
        let m = metrics(&spec, doc);
        assert_eq!(m.keyword_coverage, 2);
        assert_eq!(m.keyword_total, 2);
    }
}
