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
        &[".json"],
        Language {
            name: "JSON",
            comment: "",
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
    (
        &[".md", ".markdown", ".mkd", ".mdx"],
        Language {
            name: "Markdown",
            comment: "<!--",
        },
    ),
];

/// Detect language from a filename.
pub fn detect(filename: &str) -> Option<Language> {
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    for (patterns, lang) in LANGUAGES {
        for pattern in *patterns {
            if pattern.starts_with('.') {
                if filename.ends_with(pattern) {
                    return Some(*lang);
                }
            } else {
                // Exact basename match or prefix+dot (e.g. "Dockerfile" matches "Dockerfile.release")
                if basename == *pattern
                    || (basename.starts_with(pattern)
                        && basename.as_bytes().get(pattern.len()) == Some(&b'.'))
                {
                    return Some(*lang);
                }
            }
        }
    }
    None
}

/// Map interpreter names (from shebangs) to languages.
/// Only covers languages that have syntax-highlighting rules.
const SHEBANGS: &[(&[&str], Language)] = &[
    (
        &["sh", "bash", "zsh", "fish", "dash", "ash", "ksh"],
        Language {
            name: "Shell",
            comment: "#",
        },
    ),
    (
        &["python", "python3", "python2"],
        Language {
            name: "Python",
            comment: "#",
        },
    ),
    (
        &["node", "nodejs", "deno", "bun"],
        Language {
            name: "JavaScript",
            comment: "//",
        },
    ),
];

/// Detect language from a shebang line (the first line of file content).
pub fn detect_from_shebang(first_line: &[u8]) -> Option<Language> {
    let line = first_line.strip_prefix(b"#!")?;
    // Extract the interpreter: split on whitespace to get the command and args.
    let line = line.trim_ascii();
    let mut parts = line
        .split(|&b| b == b' ' || b == b'\t')
        .filter(|p| !p.is_empty());
    let cmd = parts.next()?;
    // If the command ends with "/env", the interpreter is the next argument.
    let interpreter = if cmd.ends_with(b"/env") {
        // Skip flags like -S
        parts.find(|p| !p.starts_with(b"-"))?
    } else {
        cmd
    };
    // Take the basename of the interpreter path.
    let basename = interpreter
        .rsplit(|&b| b == b'/')
        .next()
        .unwrap_or(interpreter);
    // Strip version suffixes (e.g. "python3.11" -> "python3")
    let name = match basename.iter().position(|&b| b == b'.') {
        Some(i) => &basename[..i],
        None => basename,
    };
    let name = std::str::from_utf8(name).ok()?;
    for (interpreters, lang) in SHEBANGS {
        if interpreters.contains(&name) {
            return Some(*lang);
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

    #[test]
    fn test_detect_dockerfile_prefix() {
        assert_eq!(detect("Dockerfile").unwrap().name, "Dockerfile");
        assert_eq!(detect("Dockerfile.release").unwrap().name, "Dockerfile");
        assert_eq!(detect("Dockerfile.dev").unwrap().name, "Dockerfile");
        assert_eq!(
            detect("/path/to/Dockerfile.prod").unwrap().name,
            "Dockerfile"
        );
    }

    #[test]
    fn test_detect_makefile_prefix() {
        assert_eq!(detect("Makefile").unwrap().name, "Makefile");
        assert_eq!(detect("/path/Makefile").unwrap().name, "Makefile");
    }

    #[test]
    fn test_shebang_direct_path() {
        assert_eq!(detect_from_shebang(b"#!/bin/bash").unwrap().name, "Shell");
        assert_eq!(detect_from_shebang(b"#!/bin/sh").unwrap().name, "Shell");
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/python3").unwrap().name,
            "Python"
        );
    }

    #[test]
    fn test_shebang_env() {
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env bash").unwrap().name,
            "Shell"
        );
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env python3").unwrap().name,
            "Python"
        );
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env node").unwrap().name,
            "JavaScript"
        );
    }

    #[test]
    fn test_shebang_env_with_flags() {
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env -S python3")
                .unwrap()
                .name,
            "Python"
        );
    }

    #[test]
    fn test_shebang_version_suffix() {
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/python3.11").unwrap().name,
            "Python"
        );
    }

    #[test]
    fn test_shebang_not_present() {
        assert!(detect_from_shebang(b"# just a comment").is_none());
        assert!(detect_from_shebang(b"print('hello')").is_none());
        assert!(detect_from_shebang(b"").is_none());
    }

    #[test]
    fn test_shebang_unknown_interpreter() {
        assert!(detect_from_shebang(b"#!/usr/bin/unknown").is_none());
    }
}
