//! Per-language syntax rule tables used by the line highlighter.

pub(crate) mod bash;
pub(crate) mod c;
pub(crate) mod css;
pub(crate) mod dockerfile;
pub(crate) mod generic;
pub(crate) mod go;
pub(crate) mod html;
pub(crate) mod ini;
pub(crate) mod javascript;
pub(crate) mod json;
pub(crate) mod makefile;
pub(crate) mod markdown;
pub(crate) mod python;
pub(crate) mod rust;
pub(crate) mod toml;
pub(crate) mod typescript;
pub(crate) mod xsh;
pub(crate) mod yaml;

pub(crate) use bash::RULES as BASH_RULES;
pub(crate) use c::RULES as C_RULES;
pub(crate) use css::RULES as CSS_RULES;
pub(crate) use dockerfile::RULES as DOCKERFILE_RULES;
pub(crate) use generic::{
    C_LIKE_RULES, DASH_RULES, ERLANG_RULES, HASH_SCRIPT_RULES, LISP_RULES, SQL_RULES,
    PLAIN_RULES, TEX_RULES,
};
pub(crate) use go::RULES as GO_RULES;
pub(crate) use html::RULES as HTML_RULES;
pub(crate) use ini::RULES as INI_RULES;
pub(crate) use javascript::RULES as JS_RULES;
pub(crate) use json::RULES as JSON_RULES;
pub(crate) use makefile::RULES as MAKEFILE_RULES;
pub(crate) use markdown::RULES as MARKDOWN_RULES;
pub(crate) use python::RULES as PYTHON_RULES;
pub(crate) use rust::RULES as RUST_RULES;
pub(crate) use toml::RULES as TOML_RULES;
pub(crate) use typescript::RULES as TS_RULES;
pub(crate) use xsh::RULES as XSH_RULES;
pub(crate) use yaml::RULES as YAML_RULES;

/// Selects the small lexer used by a static language rule table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexerKind {
    /// The generic comment/string/keyword scanner.
    Code,
    /// Generic scanning plus Rust lifetimes, raw strings, and nested comments.
    Rust,
    /// Generic scanning with Go declaration and block punctuation spans.
    Go,
    /// Generic scanning with Python declaration and receiver-name spans.
    Python,
    /// Generic scanning with TypeScript declaration spans.
    TypeScript,
    /// Generic scanning with language-specific shell interpolation.
    Bash,
    /// Generic scanning with C preprocessor directives.
    C,
    /// Generic scanning with HTML tag and attribute spans.
    Html,
    /// Generic scanning with CSS selectors, properties, and units.
    Css,
    /// Generic scanning with make variable and target spans.
    Makefile,
    /// Generic scanning with Dockerfile variable spans.
    Dockerfile,
    /// Generic scanning with JavaScript/TypeScript template interpolation.
    Script,
    /// Markdown block and inline markup scanner.
    Markdown,
    /// JSON key/value scanner.
    Json,
    /// YAML key/value scanner.
    Yaml,
    /// INI/config key/value scanner.
    Ini,
}

/// A paired string delimiter with an optional multiline flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StringDelim {
    pub(crate) open: &'static str,
    pub(crate) close: &'static str,
    pub(crate) multiline: bool,
}

/// Static rules that drive the byte-by-byte highlighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuleSet {
    /// Selects the lexical path used after line-state setup.
    pub(crate) lexer_kind: LexerKind,
    pub(crate) line_comment: &'static str,
    pub(crate) block_comment: (&'static str, &'static str),
    pub(crate) string_delims: &'static [StringDelim],
    pub(crate) keywords: &'static [&'static str],
    pub(crate) types: &'static [&'static str],
    pub(crate) constants: &'static [&'static str],
    pub(crate) macros: &'static [&'static str],
    pub(crate) operators: &'static [&'static str],
    pub(crate) highlight_numbers: bool,
    /// Highlight UPPER_SNAKE_CASE identifiers as constants.
    pub(crate) highlight_upper_constants: bool,
    /// Highlight identifiers followed by `(` as functions.
    pub(crate) highlight_fn_calls: bool,
    /// Highlight `ident!` patterns as macros (Rust-style).
    pub(crate) highlight_bang_macros: bool,
}

macro_rules! string_delim {
    ($open:expr, $close:expr, $ml:expr) => {
        StringDelim {
            open: $open,
            close: $close,
            multiline: $ml,
        }
    };
}
pub(crate) use string_delim;
