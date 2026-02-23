/// Language detection and comment syntax.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Language {
    pub name: &'static str,
    pub comment: &'static str,
}

const LANGUAGES: &[(&[&str], Language)] = &[
    (
        &[".rs"],
        Language {
            name: "Rust",
            comment: "//",
        },
    ),
    (
        &[".c", ".h"],
        Language {
            name: "C",
            comment: "//",
        },
    ),
    (
        &[".cpp", ".cc", ".cxx", ".hpp", ".hxx"],
        Language {
            name: "C++",
            comment: "//",
        },
    ),
    (
        &[".go"],
        Language {
            name: "Go",
            comment: "//",
        },
    ),
    (
        &[".js", ".jsx", ".mjs"],
        Language {
            name: "JavaScript",
            comment: "//",
        },
    ),
    (
        &[".ts", ".tsx"],
        Language {
            name: "TypeScript",
            comment: "//",
        },
    ),
    (
        &[".java"],
        Language {
            name: "Java",
            comment: "//",
        },
    ),
    (
        &[".cs"],
        Language {
            name: "C#",
            comment: "//",
        },
    ),
    (
        &[".swift"],
        Language {
            name: "Swift",
            comment: "//",
        },
    ),
    (
        &[".kt", ".kts"],
        Language {
            name: "Kotlin",
            comment: "//",
        },
    ),
    (
        &[".scala"],
        Language {
            name: "Scala",
            comment: "//",
        },
    ),
    (
        &[".py", ".pyi"],
        Language {
            name: "Python",
            comment: "#",
        },
    ),
    (
        &[".rb"],
        Language {
            name: "Ruby",
            comment: "#",
        },
    ),
    (
        &[".sh", ".bash", ".zsh", ".fish"],
        Language {
            name: "Shell",
            comment: "#",
        },
    ),
    (
        &[".pl", ".pm"],
        Language {
            name: "Perl",
            comment: "#",
        },
    ),
    (
        &[".r"],
        Language {
            name: "R",
            comment: "#",
        },
    ),
    (
        &[".yaml", ".yml"],
        Language {
            name: "YAML",
            comment: "#",
        },
    ),
    (
        &[".toml"],
        Language {
            name: "TOML",
            comment: "#",
        },
    ),
    (
        &[".conf", ".cfg", ".ini"],
        Language {
            name: "Config",
            comment: "#",
        },
    ),
    (
        &[".lua"],
        Language {
            name: "Lua",
            comment: "--",
        },
    ),
    (
        &[".sql"],
        Language {
            name: "SQL",
            comment: "--",
        },
    ),
    (
        &[".hs"],
        Language {
            name: "Haskell",
            comment: "--",
        },
    ),
    (
        &[".elm"],
        Language {
            name: "Elm",
            comment: "--",
        },
    ),
    (
        &[".html", ".htm"],
        Language {
            name: "HTML",
            comment: "<!--",
        },
    ),
    (
        &[".xml", ".svg"],
        Language {
            name: "XML",
            comment: "<!--",
        },
    ),
    (
        &[".css"],
        Language {
            name: "CSS",
            comment: "/*",
        },
    ),
    (
        &[".scss", ".sass"],
        Language {
            name: "SCSS",
            comment: "//",
        },
    ),
    (
        &[".less"],
        Language {
            name: "Less",
            comment: "//",
        },
    ),
    (
        &[".php"],
        Language {
            name: "PHP",
            comment: "//",
        },
    ),
    (
        &[".ex", ".exs"],
        Language {
            name: "Elixir",
            comment: "#",
        },
    ),
    (
        &[".erl", ".hrl"],
        Language {
            name: "Erlang",
            comment: "%",
        },
    ),
    (
        &[".clj", ".cljs"],
        Language {
            name: "Clojure",
            comment: ";;",
        },
    ),
    (
        &[".lisp", ".cl", ".el"],
        Language {
            name: "Lisp",
            comment: ";;",
        },
    ),
    (
        &[".vim"],
        Language {
            name: "Vim",
            comment: "\"",
        },
    ),
    (
        &[".zig"],
        Language {
            name: "Zig",
            comment: "//",
        },
    ),
    (
        &[".d"],
        Language {
            name: "D",
            comment: "//",
        },
    ),
    (
        &[".dart"],
        Language {
            name: "Dart",
            comment: "//",
        },
    ),
    (
        &[".m"],
        Language {
            name: "Objective-C",
            comment: "//",
        },
    ),
    (
        &[".v"],
        Language {
            name: "V",
            comment: "//",
        },
    ),
    (
        &[".nim"],
        Language {
            name: "Nim",
            comment: "#",
        },
    ),
    (
        &[".cr"],
        Language {
            name: "Crystal",
            comment: "#",
        },
    ),
    (
        &[".jl"],
        Language {
            name: "Julia",
            comment: "#",
        },
    ),
    (
        &[".tf"],
        Language {
            name: "Terraform",
            comment: "#",
        },
    ),
    (
        &["Makefile", "makefile", "GNUmakefile"],
        Language {
            name: "Makefile",
            comment: "#",
        },
    ),
    (
        &["Dockerfile"],
        Language {
            name: "Dockerfile",
            comment: "#",
        },
    ),
    (
        &[".cmake"],
        Language {
            name: "CMake",
            comment: "#",
        },
    ),
    (
        &[".proto"],
        Language {
            name: "Protobuf",
            comment: "//",
        },
    ),
    (
        &[".graphql", ".gql"],
        Language {
            name: "GraphQL",
            comment: "#",
        },
    ),
];

/// Detect language from a filename.
pub fn detect(filename: &str) -> Option<Language> {
    for (patterns, lang) in LANGUAGES {
        for pattern in *patterns {
            if pattern.starts_with('.') {
                if filename.ends_with(pattern) {
                    return Some(*lang);
                }
            } else if filename.ends_with(pattern) {
                return Some(*lang);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_extensions_resolve() {
        for (patterns, lang) in LANGUAGES {
            for pattern in *patterns {
                let filename = if pattern.starts_with('.') {
                    format!("test{pattern}")
                } else {
                    pattern.to_string()
                };
                assert_eq!(
                    detect(&filename).map(|l| l.name),
                    Some(lang.name),
                    "{filename} should detect as {}",
                    lang.name
                );
            }
        }
    }

    #[test]
    fn test_unknown_returns_none() {
        assert!(detect("readme.txt").is_none());
        assert!(detect("data.bin").is_none());
        assert!(detect("noext").is_none());
    }

    #[test]
    fn test_detect_with_path() {
        assert_eq!(detect("/some/path/main.rs").unwrap().name, "Rust");
    }
}
