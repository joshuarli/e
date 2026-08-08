//! Markdown syntax rules.

use super::SyntaxRules;

pub static RULES: SyntaxRules = SyntaxRules {
    line_comment: "",
    block_comment: ("<!--", "-->"),
    string_delims: &[],
    keywords: &[],
    types: &[],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: false,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: true,
    is_json: false,
    is_yaml: false,
    is_ini: false,
};
