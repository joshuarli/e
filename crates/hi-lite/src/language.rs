use crate::languages;

/// A built-in language lexer.
///
/// This is the stable language boundary of `hi-lite`: callers select one of
/// these small, static rule tables without constructing or depending on a
/// grammar description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    Go,
    JavaScript,
    TypeScript,
    Bash,
    C,
    Json,
    Yaml,
    Toml,
    Ini,
    Makefile,
    Html,
    Css,
    Dockerfile,
    Markdown,
    Xsh,
}

impl Language {
    /// Resolve a language name or common short alias.
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("rust") || name.eq_ignore_ascii_case("rs") {
            Some(Self::Rust)
        } else if name.eq_ignore_ascii_case("python") || name.eq_ignore_ascii_case("py") {
            Some(Self::Python)
        } else if name.eq_ignore_ascii_case("go") || name.eq_ignore_ascii_case("golang") {
            Some(Self::Go)
        } else if matches_ignore_ascii_case(name, &["javascript", "js", "jsx", "mjs"]) {
            Some(Self::JavaScript)
        } else if matches_ignore_ascii_case(name, &["typescript", "ts", "tsx"]) {
            Some(Self::TypeScript)
        } else if matches_ignore_ascii_case(name, &["bash", "shell", "sh", "zsh", "fish"]) {
            Some(Self::Bash)
        } else if name.eq_ignore_ascii_case("c") {
            Some(Self::C)
        } else if name.eq_ignore_ascii_case("json") {
            Some(Self::Json)
        } else if matches_ignore_ascii_case(name, &["yaml", "yml"]) {
            Some(Self::Yaml)
        } else if name.eq_ignore_ascii_case("toml") {
            Some(Self::Toml)
        } else if matches_ignore_ascii_case(name, &["ini", "config", "conf", "cfg"]) {
            Some(Self::Ini)
        } else if matches_ignore_ascii_case(name, &["makefile", "make"]) {
            Some(Self::Makefile)
        } else if matches_ignore_ascii_case(name, &["html", "htm"]) {
            Some(Self::Html)
        } else if name.eq_ignore_ascii_case("css") {
            Some(Self::Css)
        } else if matches_ignore_ascii_case(name, &["dockerfile", "docker"]) {
            Some(Self::Dockerfile)
        } else if matches_ignore_ascii_case(name, &["markdown", "md", "mdx"]) {
            Some(Self::Markdown)
        } else if name.eq_ignore_ascii_case("xsh") {
            Some(Self::Xsh)
        } else {
            None
        }
    }

    /// Resolve an extension with or without its leading dot.
    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension.strip_prefix('.').unwrap_or(extension);
        Self::from_name(extension)
    }

    /// Return the canonical display name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::Go => "Go",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Bash => "Bash",
            Self::C => "C",
            Self::Json => "JSON",
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
            Self::Ini => "Config",
            Self::Makefile => "Makefile",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Dockerfile => "Dockerfile",
            Self::Markdown => "Markdown",
            Self::Xsh => "XSH",
        }
    }

    pub(crate) fn rules(self) -> &'static languages::RuleSet {
        match self {
            Self::Rust => &languages::RUST_RULES,
            Self::Python => &languages::PYTHON_RULES,
            Self::Go => &languages::GO_RULES,
            Self::JavaScript => &languages::JS_RULES,
            Self::TypeScript => &languages::TS_RULES,
            Self::Bash => &languages::BASH_RULES,
            Self::C => &languages::C_RULES,
            Self::Json => &languages::JSON_RULES,
            Self::Yaml => &languages::YAML_RULES,
            Self::Toml => &languages::TOML_RULES,
            Self::Ini => &languages::INI_RULES,
            Self::Makefile => &languages::MAKEFILE_RULES,
            Self::Html => &languages::HTML_RULES,
            Self::Css => &languages::CSS_RULES,
            Self::Dockerfile => &languages::DOCKERFILE_RULES,
            Self::Markdown => &languages::MARKDOWN_RULES,
            Self::Xsh => &languages::XSH_RULES,
        }
    }
}

fn matches_ignore_ascii_case(name: &str, aliases: &[&str]) -> bool {
    aliases.iter().any(|alias| name.eq_ignore_ascii_case(alias))
}
