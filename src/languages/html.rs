//! HTML syntax rules.

use super::{StringDelim, SyntaxRules, string_delim};

static STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

pub static RULES: SyntaxRules = SyntaxRules {
    line_comment: "",
    block_comment: ("<!--", "-->"),
    string_delims: STRINGS,
    keywords: &[],
    types: &[],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: false,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
};
