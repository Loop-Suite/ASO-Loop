use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Store. Only field composition differs; validation/scoring logic is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    Apple,
    Google,
}

impl Store {
    pub fn label(&self) -> &'static str {
        match self {
            Store::Apple => "Apple App Store",
            Store::Google => "Google Play",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Spec {
    /// App/campaign name
    pub name: String,
    pub store: Store,
    /// Context such as app overview, tone & manner, etc. Inserted verbatim into the prompt.
    #[serde(default)]
    pub context: String,
    /// Notes on weighting rationale (shown in the report).
    #[serde(default)]
    pub scoring_source: String,
    /// List of target keywords. Used for coverage checks.
    #[serde(default)]
    pub target_keywords: Vec<String>,
    /// User-defined banned-term patterns such as competitor app names, trademarks, etc. (regex, case-insensitive).
    #[serde(default)]
    pub banned_terms: Vec<String>,
    /// Maximum number of allowed emojis. Flagged if exceeded.
    #[serde(default = "default_emoji_max")]
    pub emoji_max: usize,
    /// Approach angles for generation diversity.
    #[serde(default)]
    pub angles: Vec<String>,
    /// Score band descriptors (0~100). Uses defaults if not specified.
    #[serde(default)]
    pub bands: Vec<String>,
    pub sections: Vec<Section>,
    pub criteria: Vec<Criterion>,
}

/// A single ASO listing field (title/subtitle/keywords/promo_text/description, etc.).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Section {
    pub id: String,
    /// Display name matched against the document's `## Title` heading.
    pub title: String,
    #[serde(default)]
    pub guide: String,
    /// Maximum character count enforced by the store (hard limit).
    pub max_chars: usize,
    /// Recommended minimum character count. If 0, no check is performed (used for a "low utilization" warning, not enforced).
    #[serde(default)]
    pub min_chars: usize,
    #[serde(default = "default_true")]
    pub required: bool,
    /// If true, treats this field like title/subtitle/keywords — fields that Apple
    /// auto-dedups across — and checks for keyword overlap with other dedup-target fields.
    #[serde(default)]
    pub keyword_dedup_target: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Criterion {
    pub id: String,
    pub name: String,
    /// Weight. Normalized internally even if the sum isn't 1.
    pub weight: f64,
    #[serde(default)]
    pub guide: String,
}

fn default_true() -> bool {
    true
}

fn default_emoji_max() -> usize {
    3
}

pub const DEFAULT_BANDS: &[&str] = &[
    "90~100: Copy that captures both top search visibility and conversion. Target keywords are naturally woven in, the CTA is clear, and there are no policy violations.",
    "75~89: Production-ready level. Core keywords are reflected, but some phrases are flat or the localization isn't smooth.",
    "60~74: Draft level. Has the right structure, but keyword placement is scattered and conversion copy stays generic.",
    "40~59: Needs rework. Keyword reflection is shallow, dominated by spammy listing or clichéd phrases.",
    "0~39: Unusable. Numerous character-count violations or content unrelated to the review criteria.",
];

/// Generous upper bound for a spec TOML file. Real specs (see `specs/example-*.toml`) are a few
/// KB; this exists only to fail fast with a clear error on an oversized/corrupted file instead of
/// reading it entirely into memory before validation runs.
const MAX_SPEC_FILE_BYTES: u64 = 10 * 1024 * 1024;

impl Spec {
    pub fn load(path: &Path) -> Result<Spec> {
        let meta = std::fs::metadata(path)
            .with_context(|| format!("Failed to stat spec file: {}", path.display()))?;
        anyhow::ensure!(
            meta.len() <= MAX_SPEC_FILE_BYTES,
            "Spec file too large ({} bytes, max {} bytes): {} — not a plausible spec TOML file",
            meta.len(),
            MAX_SPEC_FILE_BYTES,
            path.display()
        );
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read spec file: {}", path.display()))?;
        let spec: Spec = toml::from_str(&s)
            .with_context(|| format!("Failed to parse spec TOML: {}", path.display()))?;
        anyhow::ensure!(!spec.sections.is_empty(), "sections is empty");
        anyhow::ensure!(!spec.criteria.is_empty(), "criteria is empty");
        anyhow::ensure!(
            spec.criteria
                .iter()
                .all(|c| c.weight.is_finite() && c.weight > 0.0),
            "all criteria weights must be finite and greater than 0"
        );
        anyhow::ensure!(
            spec.sections.iter().all(|s| s.max_chars > 0),
            "all sections' max_chars must be greater than 0"
        );
        let mut ids: Vec<&str> = spec.criteria.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        anyhow::ensure!(ids.len() == n, "duplicate criteria id");
        let mut section_ids: Vec<&str> = spec.sections.iter().map(|s| s.id.as_str()).collect();
        section_ids.sort_unstable();
        let n = section_ids.len();
        section_ids.dedup();
        anyhow::ensure!(section_ids.len() == n, "duplicate section id");
        // Sibling of the duplicate-id check above: field_bodies() (checks.rs) matches a section
        // to a document heading via the same normalization (alphanumeric + lowercase) used here,
        // and `.find()` only ever returns the first match. Two sections with different `id`s but
        // titles that collide after normalization (e.g. "Promo Text" vs "PromoText") would
        // silently alias to the same document body instead of erroring, so reject that upfront too.
        let mut norm_titles: Vec<String> = spec
            .sections
            .iter()
            .map(|s| crate::checks::norm_head(&s.title))
            .collect();
        norm_titles.sort_unstable();
        let n = norm_titles.len();
        norm_titles.dedup();
        anyhow::ensure!(
            norm_titles.len() == n,
            "duplicate section title (after normalizing case/punctuation) — field_bodies() would silently alias two sections to the same document body"
        );
        let bad: Vec<&str> = spec
            .banned_terms
            .iter()
            .map(|s| s.as_str())
            .filter(|p| {
                regex::RegexBuilder::new(p)
                    .case_insensitive(true)
                    .build()
                    .is_err()
            })
            .collect();
        anyhow::ensure!(
            bad.is_empty(),
            "banned_terms contains regex patterns that fail to compile: {} — blocking this upfront since silently ignoring it would remove that banned-term check",
            bad.join(", ")
        );
        Ok(spec)
    }

    pub fn weight_sum(&self) -> f64 {
        self.criteria.iter().map(|c| c.weight).sum()
    }

    pub fn bands_prompt(&self) -> String {
        if self.bands.is_empty() {
            DEFAULT_BANDS.join("\n")
        } else {
            self.bands.join("\n")
        }
    }

    pub fn sections_prompt(&self) -> String {
        self.sections
            .iter()
            .map(|s| {
                let mut line = format!(
                    "## {}\n- Writing guide: {}\n- Max {} chars",
                    s.title, s.guide, s.max_chars
                );
                if s.min_chars > 0 {
                    line.push_str(&format!(" (recommended {} chars or more)", s.min_chars));
                }
                if s.required {
                    line.push_str("\n- Required field");
                } else {
                    line.push_str("\n- Optional field (may be left empty)");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn rubric_prompt(&self) -> String {
        let sum = self.weight_sum();
        self.criteria
            .iter()
            .map(|c| {
                format!(
                    "- id=\"{}\" | {} (weight {:.0}%) : {}",
                    c.id,
                    c.name,
                    c.weight / sum * 100.0,
                    c.guide
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn keywords_prompt(&self) -> String {
        if self.target_keywords.is_empty() {
            "(no target keywords specified)".to_string()
        } else {
            self.target_keywords.join(", ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("specs")
            .join(name)
    }

    /// Writes a spec TOML string to a uniquely-named temp file (unique per test name + process id,
    /// since `cargo test` runs tests in parallel within the same process).
    fn write_temp_spec(test_name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aso_spec_test_{}_{}.toml",
            std::process::id(),
            test_name
        ));
        std::fs::write(&path, contents).expect("failed to write temp spec file");
        path
    }

    const MINIMAL_SECTION: &str = r#"
[[sections]]
id = "title"
title = "Title"
max_chars = 10
"#;

    #[test]
    fn example_apple_spec_loads_and_normalizes() {
        let sp = Spec::load(&spec_path("example-apple.toml")).expect("failed to load apple spec");
        assert_eq!(sp.store, Store::Apple);
        assert!(sp
            .sections
            .iter()
            .any(|s| s.id == "keywords" && s.max_chars == 100));
        assert!((sp.weight_sum() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn example_google_spec_loads_and_normalizes() {
        let sp = Spec::load(&spec_path("example-google.toml")).expect("failed to load google spec");
        assert_eq!(sp.store, Store::Google);
        assert!(sp
            .sections
            .iter()
            .any(|s| s.id == "short_description" && s.max_chars == 80));
        assert!((sp.weight_sum() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn load_rejects_infinite_criterion_weight() {
        // TOML supports `inf`/`nan` float literals. `inf > 0.0` is true, so a naive
        // `weight > 0.0` check alone lets it through — it must also be rejected as non-finite,
        // since it corrupts weight_sum()/rubric_prompt()/score_doc()'s total with NaN downstream.
        let toml_src = format!(
            "name = \"T\"\nstore = \"apple\"\n{}\n[[criteria]]\nid = \"x\"\nname = \"x\"\nweight = inf\n",
            MINIMAL_SECTION
        );
        let path = write_temp_spec("infinite_weight", &toml_src);
        let err = Spec::load(&path).expect_err("inf weight must be rejected");
        assert!(
            format!("{err:#}").contains("finite"),
            "error should mention finiteness: {err:#}"
        );
    }

    #[test]
    fn load_rejects_nan_criterion_weight() {
        let toml_src = format!(
            "name = \"T\"\nstore = \"apple\"\n{}\n[[criteria]]\nid = \"x\"\nname = \"x\"\nweight = nan\n",
            MINIMAL_SECTION
        );
        let path = write_temp_spec("nan_weight", &toml_src);
        assert!(Spec::load(&path).is_err(), "nan weight must be rejected");
    }

    #[test]
    fn load_rejects_duplicate_normalized_section_title() {
        // Different ids, but titles collide once normalized (alphanumeric + lowercase) the same
        // way field_bodies() normalizes headings — must be rejected, mirroring the existing
        // duplicate-section-id check.
        let toml_src = r#"
name = "T"
store = "apple"

[[sections]]
id = "promo_text"
title = "Promo Text"
max_chars = 100

[[sections]]
id = "promotext2"
title = "PromoText"
max_chars = 50

[[criteria]]
id = "x"
name = "x"
weight = 1.0
"#;
        let path = write_temp_spec("dup_normalized_title", toml_src);
        let err = Spec::load(&path).expect_err("colliding normalized titles must be rejected");
        assert!(
            format!("{err:#}").contains("duplicate section title"),
            "{err:#}"
        );
    }

    #[test]
    fn load_allows_distinct_normalized_section_titles() {
        let toml_src = format!(
            "name = \"T\"\nstore = \"apple\"\n{}\n[[sections]]\nid = \"subtitle\"\ntitle = \"Subtitle\"\nmax_chars = 20\n\n[[criteria]]\nid = \"x\"\nname = \"x\"\nweight = 1.0\n",
            MINIMAL_SECTION
        );
        let path = write_temp_spec("distinct_titles", &toml_src);
        assert!(Spec::load(&path).is_ok());
    }

    #[test]
    fn load_rejects_oversized_spec_file() {
        let path = write_temp_spec("oversized", "");
        // Overwrite with a file larger than MAX_SPEC_FILE_BYTES without materializing 10MB of
        // real TOML content in the test — content doesn't matter, only the file length does,
        // since the size check happens before parsing.
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(MAX_SPEC_FILE_BYTES + 1).unwrap();
        let err = Spec::load(&path).expect_err("oversized spec file must be rejected");
        assert!(format!("{err:#}").contains("too large"), "{err:#}");
    }
}
