//! Template rendering: conditional-block stripping plus placeholder substitution.
//!
//! Condition markers are recognized only as whole trimmed lines — `#@if EXPR`
//! opens a block, `#@endif` closes it. `#` is a comment in both bash and
//! PowerShell, so a marker retained in generated output stays syntactically
//! inert; the stripper nevertheless removes every marker line from the output.
//! Blocks do not nest; expressions support `!NAME` and `NAME && NAME` terms
//! only. Any structural violation is a [`ScaffoldError::InvalidTemplate`]
//! (build-time asset bug), never a silent mis-strip.

use super::{LocalizedTitle, ScaffoldError};

/// Strips conditional blocks and substitutes placeholders.
///
/// `vars` maps placeholder names (without the `__` fences) to replacements;
/// `conds` maps condition names to their boolean value for this shape.
pub(super) fn render(
    template: &'static str,
    vars: &[(&str, String)],
    conds: &[(&str, bool)],
) -> Result<String, ScaffoldError> {
    Ok(substitute(&strip_conditionals(template, conds)?, vars))
}

/// Substitutes `__NAME__` placeholders (no conditional handling).
pub(super) fn substitute(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (name, value) in vars {
        let token = format!("__{name}__");
        if out.contains(&token) {
            out = out.replace(&token, value);
        }
    }
    out
}

/// Builds the `__TITLE_ROWS__` replacement text: one
/// `locale: { title: 'value' },` line per localized-title row (two-space
/// indent matching the template's `registerI18n` call site, rows joined by
/// newlines, input order preserved; title values have their single quotes
/// escaped).
pub(super) fn title_rows(titles: &[LocalizedTitle]) -> String {
    titles
        .iter()
        .map(|row| {
            format!(
                "  {}: {{ title: '{}' }},",
                row.locale,
                row.value.replace('\'', "\\'")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Line-oriented conditional stripper. No nesting: an `#@if` inside a block is
/// a malformed-asset error, so a mis-annotated template fails loudly instead of
/// being silently mis-stripped.
fn strip_conditionals(
    template: &'static str,
    conds: &[(&str, bool)],
) -> Result<String, ScaffoldError> {
    let malformed = |reason: &'static str| ScaffoldError::InvalidTemplate { template, reason };
    let mut out = String::with_capacity(template.len());
    let mut inside_block = false;
    let mut skipping = false;
    for line in template.lines() {
        let trimmed = line.trim();
        if let Some(expr) = trimmed.strip_prefix("#@if ") {
            if inside_block {
                return Err(malformed("nested #@if is not supported"));
            }
            inside_block = true;
            skipping = !eval_expr(expr, conds).map_err(|()| {
                malformed("unrecognized #@if expression (expected NAME, !NAME, or NAME && NAME)")
            })?;
            continue;
        }
        if trimmed == "#@endif" {
            if !inside_block {
                return Err(malformed("#@endif without a matching #@if"));
            }
            inside_block = false;
            skipping = false;
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    if inside_block {
        return Err(malformed("#@if without a matching #@endif"));
    }
    Ok(out)
}

/// Evaluates a condition expression: `NAME` | `!NAME` (`&&`-joined terms). The
/// negation prefix binds tight (`!NAME`) or spaced (`! NAME`).
fn eval_expr(expr: &str, conds: &[(&str, bool)]) -> Result<bool, ()> {
    let mut result = true;
    let mut negate = false;
    let mut expect_term = true;
    for token in expr.split_whitespace() {
        match token {
            "&&" if !expect_term => {
                expect_term = true;
                negate = false;
            }
            "!" if expect_term => negate = !negate,
            name if expect_term => {
                let (negate, name) = match name.strip_prefix('!') {
                    Some(rest) => (!negate, rest),
                    None => (negate, name),
                };
                let value = conds
                    .iter()
                    .find(|(cond_name, _)| *cond_name == name)
                    .map(|(_, value)| *value)
                    .ok_or(())?;
                result &= value != negate;
                expect_term = false;
            }
            _ => return Err(()),
        }
    }
    if expect_term {
        return Err(());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conds() -> Vec<(&'static str, bool)> {
        vec![("HAS_SERVER", true), ("HAS_WEB", false)]
    }

    #[test]
    fn keeps_true_branch_and_drops_false_branch_with_markers() {
        let tpl = "a\n#@if HAS_SERVER\nb\n#@endif\n#@if HAS_WEB\nc\n#@endif\nd\n";
        let out = render(tpl, &[], &conds()).unwrap();
        assert_eq!(out, "a\nb\nd\n");
    }

    #[test]
    fn negation_and_conjunction_terms() {
        let tpl = "#@if !HAS_WEB\nx\n#@endif\n#@if HAS_SERVER && !HAS_WEB\ny\n#@endif\n#@if HAS_SERVER && HAS_WEB\nz\n#@endif\n";
        let out = render(tpl, &[], &conds()).unwrap();
        assert_eq!(out, "x\ny\n");
    }

    #[test]
    fn unclosed_block_is_a_template_error() {
        let err = strip_conditionals("#@if HAS_SERVER\nx\n", &conds()).unwrap_err();
        assert!(err.to_string().contains("without a matching #@endif"));
    }

    #[test]
    fn nested_block_is_a_template_error() {
        let err = strip_conditionals(
            "#@if HAS_SERVER\n#@if HAS_WEB\nx\n#@endif\n#@endif\n",
            &conds(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("nested"));
    }

    #[test]
    fn stray_endif_is_a_template_error() {
        let err = strip_conditionals("x\n#@endif\n", &conds()).unwrap_err();
        assert!(err.to_string().contains("without a matching #@if"));
    }

    #[test]
    fn unknown_condition_name_is_a_template_error() {
        let err = strip_conditionals("#@if HAS_OTHER\nx\n#@endif\n", &conds()).unwrap_err();
        assert!(err.to_string().contains("unrecognized"));
    }

    #[test]
    fn substitution_replaces_every_occurrence() {
        let out = substitute(
            "__ID__-server and __ID__ again",
            &[("ID", "demo".to_string())],
        );
        assert_eq!(out, "demo-server and demo again");
    }

    #[test]
    fn title_rows_render_one_escaped_line_per_row_in_input_order() {
        let rows = title_rows(&[
            LocalizedTitle {
                locale: "zh".to_string(),
                value: "演示组件".to_string(),
            },
            LocalizedTitle {
                locale: "en".to_string(),
                value: "Demo's Comp".to_string(),
            },
        ]);
        assert_eq!(
            rows,
            "  zh: { title: '演示组件' },\n  en: { title: 'Demo\\'s Comp' },"
        );
    }
}
