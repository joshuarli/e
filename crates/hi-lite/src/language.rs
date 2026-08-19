use crate::languages;

/// A dependency-free syntax selection.
///
/// The variants mirror the programming-language grammars shipped in syntect's
/// default syntax set. Most of them intentionally share one of the generic
/// rule tables in `languages::generic`; specialized variants retain the richer
/// scanners already present for Rust, Python, markup, and build files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust, Python, Go, JavaScript, TypeScript, Bash, C, Cpp, CSharp, Json, Yaml,
    Toml, Ini, Makefile, Html, Css, Scss, Less, Dockerfile, Markdown, Xml,
    ActionScript, AppleScript, Batch, Clojure, D, Erlang, Graphviz, Groovy,
    Haskell, Java, LaTeX, Lisp, Lua, Matlab, Ocaml, ObjectiveC, ObjectiveCpp,
    Pascal, Perl, Php, R, Ruby, Scala, Sql, Swift, Tcl, Kotlin, Elm, Regex, PlainText, Xsh,
}

impl Language {
    /// Resolve a syntax name or common alias, case-insensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        if matches_ignore_ascii_case(name, &["rust", "rs"]) {
            Some(Self::Rust)
        } else if matches_ignore_ascii_case(name, &["python", "py", "python 3"]) {
            Some(Self::Python)
        } else if matches_ignore_ascii_case(name, &["go", "golang"]) {
            Some(Self::Go)
        } else if matches_ignore_ascii_case(name, &["javascript", "js", "jsx", "mjs", "javascript (rails)"]) {
            Some(Self::JavaScript)
        } else if matches_ignore_ascii_case(name, &["typescript", "ts", "tsx"]) {
            Some(Self::TypeScript)
        } else if matches_ignore_ascii_case(name, &["bash", "shell", "sh", "zsh", "fish", "bourne again shell (bash)", "shell-unix-generic", "commands-builtin-shell-bash"]) {
            Some(Self::Bash)
        } else if name.eq_ignore_ascii_case("c") {
            Some(Self::C)
        } else if matches_ignore_ascii_case(name, &["c++", "cpp", "cxx"]) {
            Some(Self::Cpp)
        } else if matches_ignore_ascii_case(name, &["c#", "csharp", "cs"]) {
            Some(Self::CSharp)
        } else if matches_ignore_ascii_case(name, &["json", "sublime-settings", "sublime-menu", "sublime-keymap", "sublime-mousemap", "sublime-theme", "sublime-build", "sublime-project", "sublime-completions", "sublime-commands", "sublime-macro", "sublime-color-scheme"]) {
            Some(Self::Json)
        } else if matches_ignore_ascii_case(name, &["yaml", "yml"]) {
            Some(Self::Yaml)
        } else if name.eq_ignore_ascii_case("toml") {
            Some(Self::Toml)
        } else if matches_ignore_ascii_case(name, &["ini", "config", "conf", "cfg"]) {
            Some(Self::Ini)
        } else if matches_ignore_ascii_case(name, &["makefile", "make", "gnu make", "gnumakefile", "ocamlmakefile"]) {
            Some(Self::Makefile)
        } else if matches_ignore_ascii_case(name, &["html", "htm", "html (asp)", "html (erlang)", "html (rails)", "html (tcl)", "asp", "java server page (jsp)"]) {
            Some(Self::Html)
        } else if name.eq_ignore_ascii_case("css") {
            Some(Self::Css)
        } else if matches_ignore_ascii_case(name, &["scss", "sass"]) {
            Some(Self::Scss)
        } else if name.eq_ignore_ascii_case("less") {
            Some(Self::Less)
        } else if matches_ignore_ascii_case(name, &["dockerfile", "docker"]) {
            Some(Self::Dockerfile)
        } else if matches_ignore_ascii_case(name, &["markdown", "md", "mdx", "multimarkdown", "restructuredtext", "textile"]) {
            Some(Self::Markdown)
        } else if matches_ignore_ascii_case(name, &["xml", "xsd", "xslt", "tld", "dtml", "rss", "opml", "svg"]) {
            Some(Self::Xml)
        } else if name.eq_ignore_ascii_case("actionscript") {
            Some(Self::ActionScript)
        } else if name.eq_ignore_ascii_case("applescript") {
            Some(Self::AppleScript)
        } else if matches_ignore_ascii_case(name, &["batch file", "batch", "bat", "cmd"]) {
            Some(Self::Batch)
        } else if name.eq_ignore_ascii_case("clojure") {
            Some(Self::Clojure)
        } else if name.eq_ignore_ascii_case("d") {
            Some(Self::D)
        } else if name.eq_ignore_ascii_case("erlang") {
            Some(Self::Erlang)
        } else if matches_ignore_ascii_case(name, &["graphviz (dot)", "dot", "graphviz"]) {
            Some(Self::Graphviz)
        } else if name.eq_ignore_ascii_case("groovy") {
            Some(Self::Groovy)
        } else if matches_ignore_ascii_case(name, &["haskell", "literate haskell"]) {
            Some(Self::Haskell)
        } else if matches_ignore_ascii_case(name, &["java", "javadoc"]) {
            Some(Self::Java)
        } else if matches_ignore_ascii_case(name, &["latex", "tex", "bibtex", "latex log"]) {
            Some(Self::LaTeX)
        } else if name.eq_ignore_ascii_case("lisp") {
            Some(Self::Lisp)
        } else if name.eq_ignore_ascii_case("lua") {
            Some(Self::Lua)
        } else if name.eq_ignore_ascii_case("matlab") {
            Some(Self::Matlab)
        } else if matches_ignore_ascii_case(name, &["ocaml", "ocamllex", "ocamlyacc", "camlp4"]) {
            Some(Self::Ocaml)
        } else if name.eq_ignore_ascii_case("objective-c") {
            Some(Self::ObjectiveC)
        } else if name.eq_ignore_ascii_case("objective-c++") {
            Some(Self::ObjectiveCpp)
        } else if name.eq_ignore_ascii_case("pascal") {
            Some(Self::Pascal)
        } else if name.eq_ignore_ascii_case("perl") {
            Some(Self::Perl)
        } else if matches_ignore_ascii_case(name, &["php", "php source"]) {
            Some(Self::Php)
        } else if matches_ignore_ascii_case(name, &["r", "r console", "rd (r documentation)"]) {
            Some(Self::R)
        } else if matches_ignore_ascii_case(name, &["ruby", "ruby haml", "ruby on rails"]) {
            Some(Self::Ruby)
        } else if name.eq_ignore_ascii_case("scala") {
            Some(Self::Scala)
        } else if matches_ignore_ascii_case(name, &["sql", "sql (rails)"]) {
            Some(Self::Sql)
        } else if name.eq_ignore_ascii_case("swift") {
            Some(Self::Swift)
        } else if name.eq_ignore_ascii_case("tcl") {
            Some(Self::Tcl)
        } else if matches_ignore_ascii_case(name, &["kotlin", "kt", "kts"]) {
            Some(Self::Kotlin)
        } else if name.eq_ignore_ascii_case("elm") {
            Some(Self::Elm)
        } else if matches_ignore_ascii_case(name, &["regular expression", "regular expressions (javascript)", "regular expressions (python)"]) {
            Some(Self::Regex)
        } else if matches_ignore_ascii_case(name, &["plain text", "make output", "cargo build results", "diff", "nant build file", "latex log"]) {
            Some(Self::PlainText)
        } else if name.eq_ignore_ascii_case("xsh") {
            Some(Self::Xsh)
        } else {
            None
        }
    }

    /// Resolve a filename extension or conventional filename.
    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension.strip_prefix('.').unwrap_or(extension);
        if extension == "M" {
            return Some(Self::ObjectiveCpp);
        }
        if let Some(language) = Self::from_name(extension) {
            return Some(language);
        }
        if name_matches(extension, &["txt", "diff", "patch", "build"]) {
            Some(Self::PlainText)
        } else if name_matches(extension, &["asa"]) {
            Some(Self::Html)
        } else if matches_ignore_ascii_case(extension, &["c", "h"]) {
            Some(Self::C)
        } else if matches_ignore_ascii_case(extension, &["cc", "cp", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++", "inl", "ipp"]) {
            Some(Self::Cpp)
        } else if matches_ignore_ascii_case(extension, &["cs", "csx"]) {
            Some(Self::CSharp)
        } else if matches_ignore_ascii_case(extension, &["asa", "as"]) {
            Some(Self::ActionScript)
        } else if matches_ignore_ascii_case(extension, &["applescript", "script editor"]) {
            Some(Self::AppleScript)
        } else if name_matches(extension, &["clj"]) {
            Some(Self::Clojure)
        } else if name_matches(extension, &["d", "di"]) {
            Some(Self::D)
        } else if name_matches(extension, &["erl", "hrl", "emakefile"]) {
            Some(Self::Erlang)
        } else if name_matches(extension, &["dot", "gv"]) {
            Some(Self::Graphviz)
        } else if name_matches(extension, &["groovy", "gvy", "gradle"]) {
            Some(Self::Groovy)
        } else if name_matches(extension, &["hs", "lhs"]) {
            Some(Self::Haskell)
        } else if name_matches(extension, &["pyx", "pyx.in", "pxd", "pxd.in", "pxi", "pxi.in", "rpy", "cpy", "sconstruct", "sconscript", "snakefile", "wscript", "gyp", "gypi"]) {
            Some(Self::Python)
        } else if name_matches(extension, &["java", "bsh"]) {
            Some(Self::Java)
        } else if name_matches(extension, &["bib", "tex", "ltx", "sty", "cls"]) {
            Some(Self::LaTeX)
        } else if name_matches(extension, &["lisp", "cl", "clisp", "l", "mud", "el", "scm", "ss", "lsp", "fasl"]) {
            Some(Self::Lisp)
        } else if name_matches(extension, &["lua"]) {
            Some(Self::Lua)
        } else if name_matches(extension, &["matlab"]) {
            Some(Self::Matlab)
        } else if name_matches(extension, &["ml", "mli", "mll", "mly"]) {
            Some(Self::Ocaml)
        } else if extension == "m" {
            Some(Self::ObjectiveC)
        } else if name_matches(extension, &["mm"]) {
            Some(Self::ObjectiveCpp)
        } else if name_matches(extension, &["pas", "p", "dpr"]) {
            Some(Self::Pascal)
        } else if name_matches(extension, &["pl", "pm", "pod", "t"]) {
            Some(Self::Perl)
        } else if name_matches(extension, &["php", "php3", "php4", "php5", "php7", "phps", "phpt", "phtml"]) {
            Some(Self::Php)
        } else if name_matches(extension, &["r", "s", "rd"]) {
            Some(Self::R)
        } else if name_matches(extension, &["rb", "rxml", "builder", "haml"]) {
            Some(Self::Ruby)
        } else if name_matches(extension, &["scala", "sbt"]) {
            Some(Self::Scala)
        } else if name_matches(extension, &["sql", "ddl", "dml", "sql.erb"]) {
            Some(Self::Sql)
        } else if name_matches(extension, &["tcl"]) {
            Some(Self::Tcl)
        } else if name_matches(extension, &["shtml", "xhtml", "inc", "tmpl", "tpl", "yaws", "jsp", "asp", "adp", "rhtml", "erb", "rails", "html.erb"]) {
            Some(Self::Html)
        } else if name_matches(extension, &["xml", "xsd", "xslt", "tld", "dtml", "rss", "opml", "svg"]) {
            Some(Self::Xml)
        } else if name_matches(extension, &["css.erb", "css.liquid"]) {
            Some(Self::Css)
        } else if name_matches(extension, &["scss", "sass"]) {
            Some(Self::Scss)
        } else if name_matches(extension, &["less"]) {
            Some(Self::Less)
        } else if name_matches(extension, &["js.erb"]) {
            Some(Self::JavaScript)
        } else if name_matches(extension, &["re"]) {
            Some(Self::Regex)
        } else if name_matches(extension, &["rst", "rest", "textile", "mkd", "markdn"]) {
            Some(Self::Markdown)
        } else if name_matches(extension, &["sublime-syntax"]) {
            Some(Self::Yaml)
        } else if name_matches(extension, &["rprofile"]) {
            Some(Self::R)
        } else if name_matches(extension, &["appfile", "gemfile", "rakefile", "vagrantfile", "berksfile", "brewfile", "capfile", "cheffile", "deliverfile", "fastfile", "guardfile", "jirafile", "podfile", "puppetfile", "snapfile", "thorfile"]) {
            Some(Self::Ruby)
        } else {
            None
        }
    }

    /// Return the canonical display name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust", Self::Python => "Python", Self::Go => "Go",
            Self::JavaScript => "JavaScript", Self::TypeScript => "TypeScript",
            Self::Bash => "Bash", Self::C => "C", Self::Cpp => "C++",
            Self::CSharp => "C#", Self::Json => "JSON", Self::Yaml => "YAML",
            Self::Toml => "TOML", Self::Ini => "Config", Self::Makefile => "Makefile",
            Self::Html => "HTML", Self::Css => "CSS", Self::Scss => "SCSS",
            Self::Less => "Less", Self::Dockerfile => "Dockerfile",
            Self::Markdown => "Markdown", Self::Xml => "XML",
            Self::ActionScript => "ActionScript", Self::AppleScript => "AppleScript",
            Self::Batch => "Batch File", Self::Clojure => "Clojure", Self::D => "D",
            Self::Erlang => "Erlang", Self::Graphviz => "Graphviz (DOT)",
            Self::Groovy => "Groovy", Self::Haskell => "Haskell", Self::Java => "Java",
            Self::LaTeX => "LaTeX", Self::Lisp => "Lisp", Self::Lua => "Lua",
            Self::Matlab => "MATLAB", Self::Ocaml => "OCaml", Self::ObjectiveC => "Objective-C",
            Self::ObjectiveCpp => "Objective-C++", Self::Pascal => "Pascal", Self::Perl => "Perl",
            Self::Php => "PHP", Self::R => "R", Self::Ruby => "Ruby", Self::Scala => "Scala",
            Self::Sql => "SQL", Self::Swift => "Swift", Self::Tcl => "Tcl",
            Self::Kotlin => "Kotlin", Self::Elm => "Elm", Self::Regex => "Regular Expression",
            Self::PlainText => "Plain Text", Self::Xsh => "XSH",
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
            Self::Clojure | Self::Lisp => &languages::LISP_RULES,
            Self::Haskell | Self::Lua | Self::Ocaml | Self::Elm => &languages::DASH_RULES,
            Self::AppleScript => &languages::DASH_RULES,
            Self::Erlang => &languages::ERLANG_RULES,
            Self::Perl | Self::R | Self::Ruby | Self::Tcl => &languages::HASH_SCRIPT_RULES,
            Self::Sql => &languages::SQL_RULES,
            Self::LaTeX | Self::Matlab => &languages::TEX_RULES,
            Self::Regex => &languages::C_LIKE_RULES,
            Self::PlainText => &languages::PLAIN_RULES,
            Self::Xml => &languages::HTML_RULES,
            Self::Scss | Self::Less => &languages::CSS_RULES,
            Self::Cpp | Self::CSharp | Self::ActionScript | Self::Batch
            | Self::D | Self::Graphviz | Self::Groovy | Self::Java
            | Self::ObjectiveC | Self::ObjectiveCpp | Self::Pascal | Self::Php | Self::Scala
            | Self::Swift | Self::Kotlin => {
                &languages::C_LIKE_RULES
            }
        }
    }
}

fn matches_ignore_ascii_case(name: &str, aliases: &[&str]) -> bool {
    aliases.iter().any(|alias| name.eq_ignore_ascii_case(alias))
}

fn name_matches(name: &str, aliases: &[&str]) -> bool {
    aliases.iter().any(|alias| name.eq_ignore_ascii_case(alias))
}
