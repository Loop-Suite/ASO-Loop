# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-08-10

Initial release. `aso` — a Rust CLI that drafts app-store listing copy (title / subtitle /
keywords / description) and scores it with an LLM rubric via the Claude Code CLI (`claude -p`)
as its backend, with no separate API key or SDK dependency.

### Added

- `aso gen` — generate `N` angle-varied listing-copy drafts, run deterministic rule checks and
  LLM rubric scoring on each, and produce a ranked report.
- `aso score` — score an existing listing file (or directory of files) against a spec, with no
  generation.
- `aso loop` — generate → score → turn scoring feedback into a revision prompt → regenerate,
  until a target score is reached or the loop stalls (stagnation patience), then optionally
  re-score the first and best drafts with a held-out gate model that never participated in the
  loop, to catch cases where the loop's own scorer was gamed.
- TOML spec format (`spec.rs`) describing store target (Apple App Store / Google Play), fields
  and their character limits, target keywords, banned terms (regex), emoji cap, and a weighted
  scoring rubric. Bundled example specs for both stores (`specs/example-apple.toml`,
  `specs/example-google.toml`).
- Deterministic checks (`checks.rs`, no LLM involved): required-field presence, character-limit
  enforcement, target-keyword coverage, cross-field keyword duplication (Apple auto-dedups
  title/subtitle/keywords when indexing), emoji-count cap, banned-term/superlative/price-phrase
  detection, and a first-pass brief-vs-copy factual-consistency check (claims like "10만+
  다운로드" that don't appear anywhere in the original brief).
- LLM rubric scoring (`score.rs`): multiple judge models and rotating evaluation lenses per
  document, trimmed-mean aggregation across rounds, and a spread metric per criterion as a
  verdict-instability signal.
- Loop safeguards (`loop_run.rs`): a length-inflation canary (score barely moved but character
  count grew a lot → likely padding, not real improvement) and a cross-iteration drift metric
  (Jaccard similarity) flagging when the score rose but the document changed almost unrelated to
  the previous iteration.
- Markdown scoring/loop reports (`report.rs`) with per-document detail, ranking tables, and
  cumulative API cost.
- `evals/README.md` — an honest, real-cost log of every prior review round (static review +
  actual `aso` CLI executions), including the round that found nothing.

### Fixed

- Retry log printed an out-of-range attempt count (e.g. `"retry 3/2"`) — the numerator could
  exceed the denominator on the final attempt. ([#3](https://github.com/Loop-Suite/ASO-Loop/issues/3))
- The generation/judge self-scoring-bias warning printed even in `aso score`, a mode that never
  generates anything and so isn't subject to that bias. ([#2](https://github.com/Loop-Suite/ASO-Loop/issues/2))
- `aso gen -n 0` fell through to a generic "no docs produced" error instead of a clear
  validation error at the actual invalid input. ([#6](https://github.com/Loop-Suite/ASO-Loop/issues/6))
- `Spec::load` didn't reject duplicate section **titles** after the same normalization
  `field_bodies()` uses to match document headings — two sections with different `id`s but
  colliding normalized titles (e.g. `"Promo Text"` vs `"PromoText"`) silently aliased to the
  same document body. Sibling of the duplicate-`id` fix below, reachable through `title`
  instead. ([#13](https://github.com/Loop-Suite/ASO-Loop/issues/13))

### Security

- **Korean banned-term/superlative regex false positives on ordinary compound words.**
  `checks.rs`'s default superlative/price patterns (최고, 최초, 유일, 세일, 특가, and an
  unanchored `1위`) matched as free substrings — every Hangul syllable is a Unicode word
  character, so `\b` gives no useful boundary in Korean, and legitimate copy like 최고급
  (premium-grade), 유일무이 (idiom "one and only"), or 31위 (ranked 31st) was wrongly rejected
  as a banned superlative/price/ranking claim. Fixed with a `bare_korean_word()` helper
  requiring a non-Hangul character (or start/end of string) on both sides of the target word,
  plus a leading-digit guard on the `1위` pattern.
  ([#5](https://github.com/Loop-Suite/ASO-Loop/issues/5))
- `Spec::load` didn't reject duplicate section **ids** — `field_bodies()` keys its result by
  `id`, so a collision silently overwrote one section's body via `BTreeMap::insert`, applying
  the wrong `max_chars` limit to whichever body survived.
  ([#4](https://github.com/Loop-Suite/ASO-Loop/issues/4))
- `Spec::load`'s weight validation (`c.weight > 0.0`) didn't reject non-finite weights. TOML
  supports `inf` as a valid float literal, and `f64::INFINITY > 0.0` is `true`, so a
  `weight = inf` spec passed validation and then corrupted `weight_sum()` / `rubric_prompt()`
  (embedded verbatim into the real generation/judge prompts sent to `claude -p`, rendering as
  `"(weight NaN%)"`) and the final score with `NaN` throughout the rest of the pipeline. Now
  requires `is_finite()` as well. ([#12](https://github.com/Loop-Suite/ASO-Loop/issues/12))
- No file-size cap on `--spec`/`--brief`/`--input` reads: `std::fs::read_to_string()` had no
  upper bound, so a single oversized or corrupted file — the realistic case being `aso score`'s
  `--input`, which explicitly batch-processes every matching file in a directory — was read
  entirely into memory before any validation ran. Added a generous 10MB cap with a clear error;
  legitimate specs/briefs/listing docs are always far smaller (specs are a few KB;
  `description` fields cap at 4000 chars in the example specs).
  ([#14](https://github.com/Loop-Suite/ASO-Loop/issues/14))
- Audited and confirmed **not** vulnerable, no fix needed: ReDoS/catastrophic backtracking
  (the `regex` crate is automata-based, not backtracking — linear time is guaranteed regardless
  of pattern); deeply-nested malformed TOML (tested to 200,000 levels of array nesting; the
  `toml` crate rejects it with a fast, clean `Err`, not a stack overflow); path traversal (every
  path this tool writes is a fixed filename or an internally-generated label, never derived from
  spec/brief/document content).

### Changed

- Dependency bumps via Dependabot: `toml` 0.8.23 → 1.1.4+spec-1.1.0
  ([#11](https://github.com/Loop-Suite/ASO-Loop/pull/11)), `clap` 4.6.5 → 4.6.6
  ([#10](https://github.com/Loop-Suite/ASO-Loop/pull/10)), `actions/checkout` 4 → 7
  ([#9](https://github.com/Loop-Suite/ASO-Loop/pull/9)).

[0.1.0]: https://github.com/Loop-Suite/ASO-Loop/releases/tag/v0.1.0
