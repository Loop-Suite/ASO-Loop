use crate::llm::Llm;
use crate::spec::Spec;
use anyhow::Result;

pub const SYSTEM: &str = "You are an ASO (App Store Optimization) copywriter deeply familiar with the \
listing rules of App Store Connect and Google Play Console. You never exceed the character limits \
enforced by the stores, and you weave target keywords in naturally while avoiding anything that reads \
like keyword stuffing. You do not use trademarked competitor app names, unsubstantiated superlatives \
like 'best/No.1/only', or pricing/discount language.";

/// Initial generation prompt.
pub fn build_prompt(spec: &Spec, idea: &str, angle: &str) -> String {
    let mut p = String::new();
    p.push_str("# Task\nWrite a draft of the app store listing copy according to the store specifications below.\n\n");
    p.push_str(&format!(
        "## Target Store: {}\n## App: {}\n{}\n\n",
        spec.store.label(),
        spec.name,
        spec.context
    ));
    if !angle.is_empty() {
        p.push_str(&format!(
            "## Differentiation Angle for This Draft\n{}\n\n",
            angle
        ));
    }
    p.push_str(&format!("## App Overview Material\n{}\n\n", idea));
    p.push_str(&format!(
        "## Target Keywords (reflect as naturally as possible)\n{}\n\n",
        spec.keywords_prompt()
    ));
    p.push_str(&format!(
        "## Fields to Write\n{}\n\n",
        spec.sections_prompt()
    ));
    p.push_str(&format!(
        "## Review Criteria (keep these in mind while writing)\n{}\n\n",
        spec.rubric_prompt()
    ));
    p.push_str(
        "## Output Rules\n\
         - Output in markdown. Use the exact field name as a `## FieldName` heading, and write only the body text below it.\n\
         - Output only the document body, with no introduction, explanation, or meta-commentary.\n\
         - Never exceed the maximum character count for each field (count including line breaks and spaces, and verify it yourself).\n\
         - Do not use competitor app names, trademarks, unsubstantiated superlatives like 'best/No.1/only/no.1', pricing/discount language, or excessive emoji.\n\
         - If there is a keywords field, do not repeat words already used in title/subtitle (they get auto-deduped, which is wasteful).\n",
    );
    p
}

/// Regeneration prompt reflecting scoring feedback.
pub fn build_revise_prompt(
    spec: &Spec,
    idea: &str,
    prev_doc: &str,
    feedback: &str,
    weak: &str,
) -> String {
    let mut p = String::new();
    p.push_str("# Task\nImprove the listing copy draft below according to the review feedback and output the full revised text.\n\n");
    p.push_str(&format!(
        "## Target Store: {}\n## App: {}\n{}\n\n",
        spec.store.label(),
        spec.name,
        spec.context
    ));
    p.push_str(&format!("## App Overview Material\n{}\n\n", idea));
    p.push_str(&format!("## Current Draft\n{}\n\n", prev_doc));
    p.push_str(&format!(
        "## Review Feedback (must be reflected)\n{}\n\n",
        feedback
    ));
    if !weak.is_empty() {
        p.push_str(&format!(
            "## Items with Particularly Low Scores\n{}\n\n",
            weak
        ));
    }
    p.push_str(&format!(
        "## Target Keywords\n{}\n\n",
        spec.keywords_prompt()
    ));
    p.push_str(&format!("## Review Criteria\n{}\n\n", spec.rubric_prompt()));
    p.push_str(&format!(
        "## Field Structure and Character Limits to Maintain\n{}\n\n",
        spec.sections_prompt()
    ));
    p.push_str(
        "## Output Rules\n\
         - Output the entire improved document in markdown. No summary of changes or meta-commentary.\n\
         - Keep well-written parts as is, and substantively strengthen only the parts that were flagged.\n\
         - You must observe each field's character limit. Do not respond by padding meaninglessly within the limit; \
           substantively replace weak sentences instead.\n\
         - Do not force keywords in to the point that sentences become unnatural (balance readability vs. keyword density).\n",
    );
    p
}

pub fn generate(llm: &Llm, spec: &Spec, idea: &str, angle: &str) -> Result<String> {
    let prompt = build_prompt(spec, idea, angle);
    llm.text(&prompt, Some(SYSTEM))
}

pub fn revise(
    llm: &Llm,
    spec: &Spec,
    idea: &str,
    prev_doc: &str,
    feedback: &str,
    weak: &str,
) -> Result<String> {
    let prompt = build_revise_prompt(spec, idea, prev_doc, feedback, weak);
    llm.text(&prompt, Some(SYSTEM))
}

/// If angles is insufficient, fill with default angles and return n of them.
pub fn angles_for(spec: &Spec, n: usize) -> Vec<String> {
    let defaults = [
        "Place core feature and category keywords at the very front of the title, prioritizing search visibility above all.",
        "Foreground emotional benefits and the change after use, prioritizing conversion rate above all.",
        "Fill target keyword coverage as densely as possible while keeping sentences natural.",
        "Foreground differentiation points versus competing apps (features, pricing policy, UX).",
        "Avoid translationese and prioritize expressions that feel natural to local users.",
        "Prioritize conciseness and readability, keeping keyword density low.",
    ];
    let pool: Vec<String> = if spec.angles.is_empty() {
        defaults.iter().map(|s| s.to_string()).collect()
    } else {
        spec.angles.clone()
    };
    (0..n).map(|i| pool[i % pool.len()].clone()).collect()
}
