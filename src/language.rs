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
    fn test_detect_rust() {
        let lang = detect("main.rs").unwrap();
        assert_eq!(lang.name, "Rust");
        assert_eq!(lang.comment, "//");
    }

    #[test]
    fn test_detect_python() {
        let lang = detect("script.py").unwrap();
        assert_eq!(lang.name, "Python");
        assert_eq!(lang.comment, "#");
    }

    #[test]
    fn test_detect_makefile() {
        let lang = detect("Makefile").unwrap();
        assert_eq!(lang.name, "Makefile");
        assert_eq!(lang.comment, "#");
    }

    #[test]
    fn test_detect_typescript() {
        let lang = detect("app.tsx").unwrap();
        assert_eq!(lang.name, "TypeScript");
        assert_eq!(lang.comment, "//");
    }

    #[test]
    fn test_detect_lua() {
        let lang = detect("init.lua").unwrap();
        assert_eq!(lang.name, "Lua");
        assert_eq!(lang.comment, "--");
    }

    #[test]
    fn test_detect_unknown() {
        assert!(detect("readme.txt").is_none());
        assert!(detect("data.bin").is_none());
    }

    #[test]
    fn test_detect_shell_variants() {
        assert_eq!(detect("run.sh").unwrap().name, "Shell");
        assert_eq!(detect("run.bash").unwrap().name, "Shell");
        assert_eq!(detect("run.zsh").unwrap().name, "Shell");
        assert_eq!(detect("config.fish").unwrap().name, "Shell");
    }

    #[test]
    fn test_detect_c_variants() {
        assert_eq!(detect("main.c").unwrap().name, "C");
        assert_eq!(detect("header.h").unwrap().name, "C");
        assert_eq!(detect("main.cpp").unwrap().name, "C++");
    }

    #[test]
    fn test_detect_with_path() {
        let lang = detect("/some/path/main.rs").unwrap();
        assert_eq!(lang.name, "Rust");
    }
}
