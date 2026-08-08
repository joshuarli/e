//! Per-language syntax rule tables.
//!
//! The highlighter's engine lives in `crate::highlight`; this module holds the
//! `SyntaxRules` definitions.  Each language gets its own file; the XSH
//! vocabulary arrays sit in a generated region of `xsh.rs`, rewritten by
//! `examples/gen_xsh.rs` (`make gen-xsh`).  `rules_for_language` maps the
//! `language::Language::name` string to the matching rules.

pub mod bash;
pub mod c;
pub mod css;
pub mod dockerfile;
pub mod go;
pub mod html;
pub mod ini;
pub mod javascript;
pub mod json;
pub mod makefile;
pub mod markdown;
pub mod python;
pub mod rust;
pub mod toml;
pub mod typescript;
pub mod xsh;
pub mod yaml;

pub use bash::RULES as BASH_RULES;
pub use c::RULES as C_RULES;
pub use css::RULES as CSS_RULES;
pub use dockerfile::RULES as DOCKERFILE_RULES;
pub use go::RULES as GO_RULES;
pub use html::RULES as HTML_RULES;
pub use ini::RULES as INI_RULES;
pub use javascript::RULES as JS_RULES;
pub use json::RULES as JSON_RULES;
pub use makefile::RULES as MAKEFILE_RULES;
pub use markdown::RULES as MARKDOWN_RULES;
pub use python::RULES as PYTHON_RULES;
pub use rust::RULES as RUST_RULES;
pub use toml::RULES as TOML_RULES;
pub use typescript::RULES as TS_RULES;
pub use xsh::RULES as XSH_RULES;
pub use yaml::RULES as YAML_RULES;

/// A paired delimiter with an optional multiline flag.
pub struct StringDelim {
    pub open: &'static str,
    pub close: &'static str,
    pub multiline: bool,
}

/// Static rules that drive the byte-by-byte highlighter.
pub struct SyntaxRules {
    pub line_comment: &'static str,
    pub block_comment: (&'static str, &'static str),
    pub string_delims: &'static [StringDelim],
    pub keywords: &'static [&'static str],
    pub types: &'static [&'static str],
    pub constants: &'static [&'static str],
    pub macros: &'static [&'static str],
    pub operators: &'static [&'static str],
    pub highlight_numbers: bool,
    /// Highlight UPPER_SNAKE_CASE identifiers as constants.
    pub highlight_upper_constants: bool,
    /// Highlight identifiers followed by `(` as functions.
    pub highlight_fn_calls: bool,
    /// Highlight `ident!` patterns as macros (Rust-style).
    pub highlight_bang_macros: bool,
    pub is_markdown: bool,
    pub is_json: bool,
    pub is_yaml: bool,
    pub is_ini: bool,
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

/// Look up syntax rules for a language name (from `language::Language::name`).
pub fn rules_for_language(name: &str) -> Option<&'static SyntaxRules> {
    match name {
        "Rust" => Some(&RUST_RULES),
        "Python" => Some(&PYTHON_RULES),
        "Go" => Some(&GO_RULES),
        "TypeScript" => Some(&TS_RULES),
        "JavaScript" => Some(&JS_RULES),
        "Shell" => Some(&BASH_RULES),
        "C" => Some(&C_RULES),
        "TOML" => Some(&TOML_RULES),
        "JSON" => Some(&JSON_RULES),
        "YAML" => Some(&YAML_RULES),
        "Makefile" => Some(&MAKEFILE_RULES),
        "HTML" => Some(&HTML_RULES),
        "CSS" => Some(&CSS_RULES),
        "Dockerfile" => Some(&DOCKERFILE_RULES),
        "Markdown" => Some(&MARKDOWN_RULES),
        "Config" => Some(&INI_RULES),
        "XSH" => Some(&XSH_RULES),
        _ => None,
    }
}
