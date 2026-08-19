//! Markdown syntax rules.

use super::{LexerKind, RuleSet};

pub(crate) static RULES: RuleSet = RuleSet {
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
    lexer_kind: LexerKind::Markdown,
};
