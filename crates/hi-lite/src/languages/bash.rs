//! Bash syntax rules.

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
        "break", "case", "continue", "declare", "do", "done", "elif", "else", "esac", "eval",
        "exec", "exit", "export", "fi", "for", "function", "if", "in", "local", "readonly",
        "return", "set", "shift", "source", "then", "trap", "unset", "while",
    ],
    types: &["false", "true"],
    constants: &[],
    macros: &[],
    operators: &["&&", "||"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    lexer_kind: LexerKind::Code,
};
