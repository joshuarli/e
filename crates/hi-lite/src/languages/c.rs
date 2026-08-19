//! C syntax rules.

use super::{LexerKind, RuleSet, StringDelim, string_delim};

static STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

pub(crate) static RULES: RuleSet = RuleSet {
    line_comment: "//",
    block_comment: ("/*", "*/"),
    string_delims: STRINGS,
    keywords: &[
        "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern",
        "for", "goto", "if", "inline", "register", "restrict", "return", "sizeof", "static",
        "struct", "switch", "typedef", "union", "volatile", "while",
    ],
    types: &[
        "NULL", "bool", "char", "double", "false", "float", "int", "int16_t", "int32_t", "int64_t",
        "int8_t", "long", "short", "signed", "size_t", "true", "uint16_t", "uint32_t", "uint64_t",
        "uint8_t", "unsigned", "void",
    ],
    constants: &[],
    macros: &[],
    operators: &["&&", "||", "!=", "==", "<=", ">=", "->"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
    lexer_kind: LexerKind::Code,
};
