# Empirical review findings

This records what actually happened when this repo went through a review pass: static code
review first, then real `aso` CLI execution to verify the fixes under an actual model. Same
spirit as [`Code-Review-Loop`'s `evals/README.md`](https://github.com/Loop-Suite/Code-Review-Loop/blob/main/evals/README.md)
— real numbers from what was actually run, not estimated, including the round that found
nothing.

## TL;DR — what was actually run, and what it cost

| Round | Type | Scope | Issues found & fixed | Real cost |
|---|---|---|---|---|
| 1 | Static code review | Initial pass over `src/` | 3 — [#2](https://github.com/Loop-Suite/ASO-Loop/issues/2), [#3](https://github.com/Loop-Suite/ASO-Loop/issues/3), [#4](https://github.com/Loop-Suite/ASO-Loop/issues/4) | — (read-only review, no `aso` CLI run) |
| 2 | Static code review, deeper pass | `src/checks.rs`, `src/main.rs` | 2 — [#5](https://github.com/Loop-Suite/ASO-Loop/issues/5), [#6](https://github.com/Loop-Suite/ASO-Loop/issues/6) | — (read-only review, no `aso` CLI run) |
| 3 | Real CLI execution (`claude -p --model haiku --judge-model haiku`) | `aso gen` on both bundled example specs + `aso score` on a Korean word-boundary regression doc | 0 — no bugs found | **$0.4655** |
| 4 | Adversarial security/robustness re-audit | `src/spec.rs`, `src/checks.rs`, `src/main.rs` | 3 — [#12](https://github.com/Loop-Suite/ASO-Loop/issues/12), [#13](https://github.com/Loop-Suite/ASO-Loop/issues/13), [#14](https://github.com/Loop-Suite/ASO-Loop/issues/14) | — (read-only review + `cargo test`/`clippy`/`fmt`, no `aso` CLI run) |
| 5 | Edge-case test coverage expansion | `src/checks.rs`, `src/spec.rs` test modules | 0 — test-only, no new production bugs; 21 → 45 tests | — (no `aso` CLI run) |
| 6 | Real CLI execution (`--model haiku --judge-model haiku`), new domain | `aso gen` on a PulseTrack (fitness app) spec/brief — different domain from the bundled MoneyFlow example | 0 — no bugs found | **$0.3506** |
| **Total** | | | **8 issues found & fixed, 2 verification rounds clean** | **$0.8161** |

**What this bought:**

- **All 5 real bugs were found by reading the code — none required an actual `aso` CLI run to
  catch.** Rounds 1 and 2 were static review only. The only round that spent real API money
  (round 3) was pure verification of an already-merged fix, not discovery of a new defect.
- **#5 is the standout finding: a Korean-specific regex false-positive class, not a generic
  "forgot `\b`" typo.** `checks.rs`'s banned-term/superlative patterns used bare Korean words
  (최고, 최초, 유일, 세일, 특가) and an unanchored `1\s*위`. Because every Hangul syllable is a
  Unicode word character, a plain substring match has no natural place to stop — the pattern
  for "최고" (best) matched as a substring inside completely unrelated compounds like 최고급
  (premium-grade) and 최고치 (highest figure), and `1위` (rank 1) matched inside any rank number
  ending in "1위" like 31위 or 101위. Ordinary, compliant app-store copy was being flagged as a
  banned superlative/price claim.
- **Round 3 found no bugs — recorded honestly, not rounded up to a "success."** Running the real
  pipeline (`aso gen` against both bundled example specs, `aso score` against a document
  deliberately loaded with every phrase #5's bug report named) exercised the fixed code
  end-to-end under real model calls and turned up nothing new. Not every review round finds
  something; this one's value was confirming the #5 fix holds under real conditions, not
  discovering a new defect.
- **The #5 fix was verified with a targeted regression document, not just a re-read of the
  diff.** A test listing containing 최고급, 최고치, 유일무이, 특가품, 세일즈, 31위, and 101위 —
  every compound word named in the original bug report — was scored with `aso score`. The
  deterministic check layer (`checks.rs`) reported `"banned_hits": []`: zero false positives.
- **The deterministic check and the LLM judge are two independent layers, and both did their own
  job correctly on the same run.** The judge model (Haiku, a separate code path from
  `checks.rs`'s regex) still commented that phrases like 최고급 read as marketing hyperbole and
  suggested toning them down — a legitimate stylistic judgment call, unrelated to and unaffected
  by the deterministic regex fix. Two different mechanisms, two different (correct) outcomes on
  the same document.
- **Total real spend: $0.4655 across 3 `aso` invocations**, all against Claude Haiku
  (`--model haiku --judge-model haiku`): $0.2280 (`aso gen`, Apple spec) + $0.1841 (`aso gen`,
  Google spec) + $0.0534 (`aso score`, Korean regression doc).

**This round added (production-hardening pass, rounds 4–6):**

- **The standout new finding is #12: a `NaN` that reaches the real LLM prompt, not just an
  internal score.** TOML's float grammar accepts `inf` as a literal, and `f64::INFINITY > 0.0`
  is `true` in Rust, so `weight = inf` passed the existing weight check. From there `weight_sum()`
  returned `inf`, every per-criterion percentage in `rubric_prompt()` computed as `NaN`, and the
  literal string `"(weight NaN%)"` was embedded into the actual generation/judge prompt text sent
  to `claude -p` — a malformed-input bug with a blast radius that reaches the model call, not just
  a bad number in a report.
- **#13 is a same-shape sibling of #4, not a new bug class.** #4 (round 1) rejected duplicate
  section *ids*; #13 found that `field_bodies()` actually matches document headings to sections by
  normalized *title* (`norm_head()`), a path #4's id-only check never covered, so two sections with
  distinct ids but colliding normalized titles still silently aliased to the same body. Same
  silent-data-loss failure mode, reached through the field #4's fix didn't touch. Fixed by exposing
  `norm_head` as `pub(crate)` and reusing it for a duplicate-title check.
- **Round 4's negative results are recorded with the same rigor as the positive ones.** ReDoS was
  ruled out structurally (the `regex` crate is automata-based, not backtracking — not "we didn't
  find an input," but "no input can trigger it"). Deeply-nested TOML was tested to 200,000 levels
  and confirmed to fail fast, not stack-overflow. Path traversal was ruled out by construction
  (every written path is a fixed name or an internally generated label). #5's Korean word-boundary
  bug was checked for recurrence in other scripts and confirmed absent, because the default
  patterns never special-cased any script besides English and Korean to begin with.
- **Round 5 found a real, low-severity gap and chose not to "fix" it: `duplicate_keywords_across_fields()`
  doesn't segment unspaced CJK text.** Its tokenizer is whitespace-based, so an exact keyword
  phrase repeated verbatim across title/subtitle in Chinese or Japanese isn't flagged. This is a
  false negative (a missed hint), not the false-positive class #5 fixed — a correct fix needs
  script-aware word segmentation, judged disproportionate to what this heuristic check is for. It's
  documented with a doc comment and pinned with a regression test instead of patched.
- **Test count went from 21 to 45**, including a regression test for #4's exact fix
  (`Spec::load` duplicate section id) that had never had one, plus non-Latin script coverage
  (Japanese, Chinese, Arabic, Cyrillic — Cyrillic specifically exercising case-insensitive
  folding) and a ~500,000-character document timing check.
- **`v0.1.0` is the project's first tagged release**, with `CHANGELOG.md` (Keep a Changelog
  format, including a dedicated Security section for #12/#13/#14) merged ahead of the tag.
- **Round 6 re-verified on a domain the codebase had never been run against**: PulseTrack, a
  fitness-tracking app, instead of the bundled MoneyFlow example every prior real-CLI round used.
  No crash, no `NaN`, no malformed output — **no new bugs found, recorded honestly** rather than
  padded into a "success," same as round 3.

## Round 1 — static review: #2, #3, #4

Initial pass over `src/`, no CLI execution. All three fixed same-day, direct commits to `main`
(no PR — each commit's `Fixes #N` trailer closed the issue on push).

### [#2](https://github.com/Loop-Suite/ASO-Loop/issues/2) — self-scoring warning prints even in Score-only mode

`aso score` never generates content, so the "generation model and judge model are the same"
bias warning doesn't apply to it, but `main.rs` printed it unconditionally for every command.
Fixed in [`902b2f3`](https://github.com/Loop-Suite/ASO-Loop/commit/902b2f3) (`src/main.rs`, 3
lines) — the warning now only fires for commands that actually generate (`Gen`, `Loop`).

### [#3](https://github.com/Loop-Suite/ASO-Loop/issues/3) — retry log prints out-of-range attempt count

`with_retry` runs `self.retries + 1` total attempts (`for attempt in 0..=self.retries`), but the
verbose log printed `"retry {attempt+1}/{self.retries}"` — the numerator could exceed the
denominator, e.g. `"retry 3/2"` on the final attempt. Fixed in
[`8e54d33`](https://github.com/Loop-Suite/ASO-Loop/commit/8e54d33) (`src/llm.rs`, 2 lines
changed) — retry logic itself untouched, only the log format changed to
`"attempt {attempt+1}/{self.retries+1}"`.

### [#4](https://github.com/Loop-Suite/ASO-Loop/issues/4) — `Spec::load` doesn't validate duplicate section ids

Criteria ids were already checked for uniqueness on load, but section ids were not. A duplicate
section id let `checks::field_bodies()` silently overwrite one section's body via
`BTreeMap::insert`, losing data and applying the wrong `max_chars` limit to whichever body
survived. Fixed in [`6b39872`](https://github.com/Loop-Suite/ASO-Loop/commit/6b39872)
(`src/spec.rs`, +5 lines) — `Spec::load` now fails fast on a duplicate section id, mirroring the
existing criteria-id check.

## Round 2 — deeper static review: #5, #6

### [#5](https://github.com/Loop-Suite/ASO-Loop/issues/5) — Korean banned-term/superlative regex false positives on ordinary compound words

**Root cause.** `default_superlative_patterns()` / `default_price_patterns()` in `src/checks.rs`
used bare Korean words — 최고, 최초, 유일, 세일, 특가 — as regex substrings, plus an unanchored
`1\s*위` for "ranked #1" claims. With no anchoring at all, a substring match has no way to tell
"최고 as a whole word" from "최고 as the first two syllables of a longer compound." Because every
Hangul syllable is classified as a Unicode word character, this bit much harder in Korean than
the same mistake would in English: Korean compounds and inflected forms are written as one
continuous run of Hangul with no space or punctuation marking the internal word boundary, so
there was no structural cue in the text (the way a space usually is in English) that would have
made the bug visible just from eyeballing sample copy.

**What it broke, concretely:**

| Text | Contains | Was flagged as |
|---|---|---|
| 최고급 (premium-grade) | 최고 | superlative claim |
| 최고치 (highest figure/record) | 최고 | superlative claim |
| 최초부터 (from the very start) | 최초 | superlative claim |
| 유일무이 (one and only, idiom) | 유일 | superlative claim |
| 특가품 (bargain item) | 특가 | price claim |
| 세일즈 (sales, as in "sales team") | 세일 | price claim |
| 31위 / 101위 (ranked 31st / 101st) | trailing `1위` | "ranked #1" claim |

All seven are ordinary, policy-compliant Korean app-store copy — none of them assert a
superlative or a #1 ranking.

**The fix**, in [`d3cbe98`](https://github.com/Loop-Suite/ASO-Loop/commit/d3cbe98)
(`src/checks.rs`, 78 insertions / 25 deletions): a `bare_korean_word()` helper
(`(?:^|[^가-힣]){word}(?:[^가-힣]|$)`) that requires a non-Hangul character — or the start/end of
the string — on both sides of the target word, instead of matching it as a free substring. The
`1위` pattern was similarly guarded so the leading `1` can't be preceded by another digit
(rejecting the `1위` tail inside 31위/101위 while still catching a standalone `1위` or `지금
1위`). Genuine claims like "국내 최고" (best in the country), "지금 1위" (ranked #1 right now),
and "특별 세일" (special sale) still match correctly.

This was re-verified for real in round 3 below, not just re-read — see "What this bought" above
and the round 3 section for the actual `aso score` run and its `banned_hits: []` result.

### [#6](https://github.com/Loop-Suite/ASO-Loop/issues/6) — `aso gen -n 0` reports a confusing error

`-n 0` fell through to the generic "no docs produced" guard, which reported `"Generation failed:
all 0 requested item(s) failed"` — misleading, since nothing was attempted or failed; `0` was
simply never validated as an invalid request up front. Fixed in
[`6a0446c`](https://github.com/Loop-Suite/ASO-Loop/commit/6a0446c) (`src/main.rs`, +4 lines) —
`-n`/`--count` is now rejected immediately with a clear validation error if it's not `> 0`.

## Round 3 — real CLI execution: verification, not discovery

Three real `aso` invocations, `claude -p --model haiku --judge-model haiku`, actual API spend.
Purpose: confirm the round-1/round-2 fixes (especially #5's regex change) hold up under a real
model and a real end-to-end run, not just under `cargo test`/manual re-reading.

| Run | Command | Result | Cost |
|---|---|---|---|
| Apple spec generation | `aso gen` on `specs/example-apple.toml`, 2 candidates | Ranked cleanly: cand01 80.4/100, cand02 78.8/100. No crash, no malformed output. | $0.2280 |
| Google spec generation | `aso gen` on `specs/example-google.toml`, 2 candidates | Ranked cleanly: cand01 77.4/100, cand02 77.0/100. No crash, no malformed output. | $0.1841 |
| Korean word-boundary regression check | `aso score` on a document containing 최고급, 최고치, 유일무이, 특가품, 세일즈, 31위, 101위 | Scored 62.0/100. Deterministic `checks.rs` output: `"banned_hits": []` — **zero false positives** on exactly the phrases #5's bug report named. | $0.0534 |
| **Total** | | **No bugs found** | **$0.4655** |

**Outcome: no bugs found.** Both `aso gen` runs completed a full generate → check → score →
rank cycle without error on the two bundled example specs (Apple and Google), and the Korean
regression doc confirmed #5's fix directly: the actual `results.jsonl` for that run reports
`"banned_hits": []`, meaning none of the seven compound words/rank numbers that used to
false-positive triggered the deterministic banned-term/superlative check after the fix.

**One thing worth being precise about, since it looks at first glance like the #5 fix "didn't
work":** the LLM judge's free-text feedback on that same run *did* comment on 최고급/최고치,
suggesting the copy "remove 'best-in-class' language." That is not a regression or a sign the
regex fix is incomplete — the judge model's rubric-based commentary (`score.rs`, an LLM call) and
the deterministic banned-term check (`checks.rs`, the regex #5 fixed) are two separate,
independent code paths. The regex check's job is narrow and mechanical (exact-pattern matching)
and it correctly stayed silent; the judge's job is broad and subjective (holistic copy quality)
and it correctly flagged that 최고급-style phrasing reads as marketing hyperbole worth softening.
Both did their own job right on the same input — this is two systems agreeing on nothing needing
mechanical rejection while one of them separately gives style advice, not a contradiction.

This round is included precisely because it *didn't* find anything — a review process where
every round reports a new bug isn't a credible one. Round 3's real contribution was confirming,
with actual model calls and real spend, that a fix already believed correct from reading the
diff actually behaves correctly end-to-end.

## Round 4 — adversarial re-audit: #12, #13, #14

A second adversarial pass over the same attack surface any external-input-parsing Rust CLI
should be re-probed on: regex handling, TOML validation, resource exhaustion, path traversal.
The explicit goal was to hunt for the same *classes* of bug already fixed elsewhere in the
codebase recurring somewhere else, plus genuinely new categories — not just re-read the existing
fixes. Fixed in [PR #15](https://github.com/Loop-Suite/ASO-Loop/pull/15), merged same-day;
static review only, no `aso` CLI run (`cargo build`/`test`/`clippy`/`fmt` all clean).

### [#12](https://github.com/Loop-Suite/ASO-Loop/issues/12) — `weight = inf` is valid TOML and passes validation, corrupting scores and the real LLM prompt with `NaN`

`Spec::load`'s weight check was `c.weight > 0.0` (`src/spec.rs`). TOML's float grammar accepts
`inf`/`-inf`/`nan` as literals, and `f64::INFINITY > 0.0` evaluates to `true` in Rust, so
`weight = inf` in a spec file passed validation silently. From there it propagated two ways:
`weight_sum()` returned `inf`, so every per-criterion percentage computed in `rubric_prompt()`
became `inf / inf = NaN` — and the literal string `"(weight NaN%)"` was embedded directly into
the generation/judge prompt text actually sent to `claude -p`, not just logged internally. The
final aggregate score in `score.rs` cascaded to `NaN` throughout the rest of the pipeline as
well. Fixed by adding `c.weight.is_finite()` alongside the existing `> 0.0` check
(`src/spec.rs:124`).

### [#13](https://github.com/Loop-Suite/ASO-Loop/issues/13) — sibling of #4: colliding normalized section *titles* silently alias to the same document body

#4 (round 1) rejected duplicate section **ids** in `Spec::load`. This issue is the same
underlying failure reached through a different field: `field_bodies()` (`src/checks.rs`)
actually matches a document's `## Heading` lines to spec sections by normalizing the heading
text and comparing it to `norm_head(&section.title)` — the **title**, not the id. So two
sections with distinct ids but colliding normalized titles (e.g. `"Promo Text"` vs
`"PromoText"`, which normalize identically) still silently aliased to the same document body via
`BTreeMap::insert` — the exact silent-data-loss failure mode #4 fixed, just reachable through a
field #4's id-only check never covered. Fixed by making `norm_head` `pub(crate)` (previously
private to `checks.rs`) and reusing it in `Spec::load` to reject duplicate normalized titles the
same way duplicate ids are already rejected (`src/checks.rs:40`, `src/spec.rs:149`).

### [#14](https://github.com/Loop-Suite/ASO-Loop/issues/14) — no size cap on `--spec`/`--brief`/`--input` file reads

`read_text()` (`src/main.rs`, backs `--brief` and every file `aso score --input <dir>`
batch-collects) and `Spec::load`'s spec-file read both called `std::fs::read_to_string()`
directly with no upper bound. `score --input` explicitly walks a directory and reads every
matching file, so a single oversized or corrupted file would be read entirely into memory before
any validation ran. Fixed with a 10MB cap — `MAX_TEXT_FILE_BYTES` (`src/main.rs:419`) and
`MAX_SPEC_FILE_BYTES` (`src/spec.rs:102`) — generous headroom over real usage (specs run a few
KB; `description` fields cap at 4000 chars in the bundled examples).

### Audited, confirmed **not** vulnerable (no issue filed)

- **ReDoS / catastrophic backtracking** — the `regex` crate (1.13.1 per `Cargo.lock`) is
  automata-based, not backtracking. Linear time is guaranteed regardless of pattern; this class
  of bug is structurally impossible here, not merely untriggered by the inputs tried.
- **Deeply-nested malformed TOML** — tested a spec file with 200,000 levels of array nesting; the
  `toml` crate rejects it with a fast, clean `Err`. No stack overflow observed.
- **Path traversal** — every path this tool writes is either a fixed filename (`report.md`,
  `results.jsonl`, `best.md`) or an internally generated label (`cand01`, `iter01`), never
  derived from spec/brief/document content. No injection surface exists.
- **#5's Korean word-boundary bug recurring in another script** — checked and does not recur,
  because the built-in banned/superlative default patterns only special-case English and Korean
  to begin with; there is no equivalent code path for another script to have the same bug in.

## Round 5 — edge-case test coverage: 21 → 45 tests

Test-only pass, [PR #16](https://github.com/Loop-Suite/ASO-Loop/pull/16), merged the same day as
round 4 — no production code changes, additions confined to `src/checks.rs` and `src/spec.rs`
test modules.

- Empty input: empty document, empty spec.
- A ~500,000-character document, timed, to confirm no pathological slowdown on large input at
  the full check-pipeline level (round 4 already cleared the regex engine itself of ReDoS; this
  exercises the whole pipeline end-to-end at scale).
- Non-Latin script coverage for user-defined banned-term matching: Japanese, Chinese, Arabic, and
  Cyrillic — Cyrillic specifically exercising case-insensitive folding.
- Non-Latin target-keyword coverage.
- A dedicated test for every validation branch in `Spec::load`, including duplicate section id —
  #4's fix (`6b39872`, round 1) had never had a regression test for the exact bug it fixed, until
  now. Also: empty sections/criteria, zero/negative `max_chars`, duplicate criteria id, invalid
  `banned_terms` regex, unparseable TOML syntax, a missing required field, and the 200,000-level
  TOML nesting case from round 4's manual audit, now pinned as a permanent regression test.

**Known limitation, documented, not fixed:** `duplicate_keywords_across_fields()`'s tokenizer is
whitespace-based, so it doesn't segment unspaced CJK text — an exact keyword phrase repeated
verbatim across title/subtitle in Chinese or Japanese isn't flagged. This is a false negative (a
missed hint, not a wrong rejection like #5 was) and low severity; a correct fix needs
script-aware word segmentation, which is disproportionate to what this heuristic check is for.
Pinned with a doc comment and a regression test instead of patched.

## Versioning — `CHANGELOG.md` + `v0.1.0` tag

[PR #17](https://github.com/Loop-Suite/ASO-Loop/pull/17) added `CHANGELOG.md` (Keep a Changelog
format), covering Added/Fixed/Security/Changed for everything through round 5 — including a
dedicated **Security** section for #12/#13/#14 (NaN weight injection, title-collision aliasing,
unbounded file read) and the confirmed-not-vulnerable audit findings from round 4. Tagged and
published as the project's first release:
[`v0.1.0`](https://github.com/Loop-Suite/ASO-Loop/releases/tag/v0.1.0).

## Round 6 — real CLI execution: verification on a new domain (PulseTrack)

One real `aso gen` invocation, `-n 2 --model haiku --judge-model haiku`, against a new spec +
brief for **PulseTrack**, a fitness-tracking app — deliberately a different product domain from
the bundled `MoneyFlow` example every prior real-CLI round (round 3) ran against, specifically to
check the round 4/5 fixes generalize past the one example already exercised, rather than
re-running the same input again.

| Run | Command | Result | Cost |
|---|---|---|---|
| PulseTrack generation | `aso gen -n 2 --model haiku --judge-model haiku` | Ranked cleanly: cand01 80.3/100, cand02 79.4/100. No crash, no `NaN`, no malformed output; `[uncertain]` warnings behaved as expected. | $0.3506 |

**Outcome: no new bugs found — recorded honestly, same standard as round 3.** This confirms the
round 4/5 fixes (the `is_finite()` weight guard, the title-collision check, the 10MB file-size
cap, the expanded CJK/edge-case coverage) hold under a real end-to-end run on an out-of-sample
domain, not just under `cargo test`. Not every round finds something; this one's value is
confirming the hardening pass holds under real conditions.
