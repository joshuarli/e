//! Makefile syntax rules.

use super::{LexerKind, RuleSet, StringDelim, string_delim};

static STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

pub(crate) static RULES: RuleSet = RuleSet {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: STRINGS,
    keywords: &[
        "define", "else", "endef", "endif", "export", "ifdef", "ifeq", "ifndef", "ifneq",
        "include", "override", "unexport", "vpath",
    ],
    types: &[],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: false,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    lexer_kind: LexerKind::Code,
};
