/// Command registry: maps command names to functions.
use std::collections::HashMap;

/// Parse a command argument string into tokens, respecting single and double quotes.
/// Unquoted tokens are split on whitespace. Quoted tokens preserve interior whitespace
/// and the quotes are stripped. Backslash-escaping is NOT supported (keep it simple).
pub fn parse_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            chars.next(); // consume opening quote
            let mut token = String::new();
            while let Some(&ch) = chars.peek() {
                if ch == quote {
                    chars.next(); // consume closing quote
                    break;
                }
                token.push(ch);
                chars.next();
            }
            args.push(token);
        } else {
            let mut token = String::new();
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() {
                    break;
                }
                token.push(ch);
                chars.next();
            }
            args.push(token);
        }
    }
    args
}

pub type CommandFn = fn(&str, &mut CommandContext);

/// Context passed to command functions so they can affect editor state.
pub struct CommandContext {
    pub action: CommandAction,
}

/// What the command wants the editor to do.
pub enum CommandAction {
    None,
    Save,
    SaveAs(String),
    Quit,
    Goto(usize),
    ToggleRuler,
    ReplaceAll {
        pattern: String,
        replacement: String,
    },
    ToggleComment,
    CommentOn,
    CommentOff,
    Find(String),
    SelectAll,
    Trim,
    TabsToSpaces,
    SpacesToTabs,
    StatusMsg(String),
}

pub struct CommandRegistry {
    commands: HashMap<String, CommandFn>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut commands: HashMap<String, CommandFn> = HashMap::new();

        commands.insert("save".to_string(), cmd_save);
        commands.insert("quit".to_string(), cmd_quit);
        commands.insert("q".to_string(), cmd_quit);
        commands.insert("goto".to_string(), cmd_goto);
        commands.insert("ruler".to_string(), cmd_ruler);
        commands.insert("find".to_string(), cmd_find);
        commands.insert("replaceall".to_string(), cmd_replaceall);
        commands.insert("comment".to_string(), cmd_comment);
        commands.insert("selectall".to_string(), cmd_selectall);
        commands.insert("trim".to_string(), cmd_trim);
        commands.insert("tabstospaces".to_string(), cmd_tabstospaces);
        commands.insert("spacestotabs".to_string(), cmd_spacestotabs);

        Self { commands }
    }

    /// Return sorted list of unique command names.
    pub fn command_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.commands.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    pub fn execute(&self, input: &str) -> CommandAction {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return CommandAction::None;
        }

        let (name, args) = match trimmed.split_once(' ') {
            Some((n, a)) => (n, a),
            None => (trimmed, ""),
        };

        if let Some(func) = self.commands.get(name) {
            let mut ctx = CommandContext {
                action: CommandAction::None,
            };
            func(args, &mut ctx);
            ctx.action
        } else {
            CommandAction::StatusMsg(format!("Unknown command: {}", name))
        }
    }
}

fn cmd_save(args: &str, ctx: &mut CommandContext) {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        ctx.action = CommandAction::Save;
    } else {
        ctx.action = CommandAction::SaveAs(trimmed.to_string());
    }
}

fn cmd_quit(_args: &str, ctx: &mut CommandContext) {
    ctx.action = CommandAction::Quit;
}

fn cmd_goto(args: &str, ctx: &mut CommandContext) {
    let trimmed = args.trim();
    if let Ok(n) = trimmed.parse::<usize>() {
        ctx.action = CommandAction::Goto(n);
    } else {
        ctx.action = CommandAction::StatusMsg("Usage: goto <line>".to_string());
    }
}

fn cmd_ruler(_args: &str, ctx: &mut CommandContext) {
    ctx.action = CommandAction::ToggleRuler;
}

fn cmd_find(args: &str, ctx: &mut CommandContext) {
    let parsed = parse_args(args);
    if parsed.is_empty() {
        ctx.action = CommandAction::StatusMsg("Usage: find <pattern>".to_string());
        return;
    }
    ctx.action = CommandAction::Find(parsed[0].clone());
}

fn cmd_replaceall(args: &str, ctx: &mut CommandContext) {
    let parsed = parse_args(args);
    if parsed.len() < 2 {
        ctx.action =
            CommandAction::StatusMsg("Usage: replaceall <pattern> <replacement>".to_string());
        return;
    }
    ctx.action = CommandAction::ReplaceAll {
        pattern: parsed[0].clone(),
        replacement: parsed[1].clone(),
    };
}

fn cmd_comment(args: &str, ctx: &mut CommandContext) {
    match args.trim() {
        "on" => ctx.action = CommandAction::CommentOn,
        "off" => ctx.action = CommandAction::CommentOff,
        "" => ctx.action = CommandAction::ToggleComment,
        _ => ctx.action = CommandAction::StatusMsg("Usage: comment [on|off]".to_string()),
    }
}

fn cmd_selectall(_args: &str, ctx: &mut CommandContext) {
    ctx.action = CommandAction::SelectAll;
}

fn cmd_trim(_args: &str, ctx: &mut CommandContext) {
    ctx.action = CommandAction::Trim;
}

fn cmd_tabstospaces(_args: &str, ctx: &mut CommandContext) {
    ctx.action = CommandAction::TabsToSpaces;
}

fn cmd_spacestotabs(_args: &str, ctx: &mut CommandContext) {
    ctx.action = CommandAction::SpacesToTabs;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        let reg = CommandRegistry::new();
        assert!(matches!(reg.execute(""), CommandAction::None));
        assert!(matches!(reg.execute("   "), CommandAction::None));
    }

    #[test]
    fn test_save_no_args() {
        let reg = CommandRegistry::new();
        assert!(matches!(reg.execute("save"), CommandAction::Save));
    }

    #[test]
    fn test_save_with_filename() {
        let reg = CommandRegistry::new();
        match reg.execute("save foo.txt") {
            CommandAction::SaveAs(name) => assert_eq!(name, "foo.txt"),
            _ => panic!("expected SaveAs"),
        }
    }

    #[test]
    fn test_quit() {
        let reg = CommandRegistry::new();
        assert!(matches!(reg.execute("quit"), CommandAction::Quit));
        assert!(matches!(reg.execute("q"), CommandAction::Quit));
    }

    #[test]
    fn test_goto_valid() {
        let reg = CommandRegistry::new();
        match reg.execute("goto 42") {
            CommandAction::Goto(n) => assert_eq!(n, 42),
            _ => panic!("expected Goto"),
        }
    }

    #[test]
    fn test_goto_invalid() {
        let reg = CommandRegistry::new();
        match reg.execute("goto abc") {
            CommandAction::StatusMsg(msg) => assert!(msg.contains("Usage")),
            _ => panic!("expected StatusMsg"),
        }
    }

    #[test]
    fn test_goto_no_args() {
        let reg = CommandRegistry::new();
        match reg.execute("goto") {
            CommandAction::StatusMsg(msg) => assert!(msg.contains("Usage")),
            _ => panic!("expected StatusMsg"),
        }
    }

    #[test]
    fn test_ruler() {
        let reg = CommandRegistry::new();
        assert!(matches!(reg.execute("ruler"), CommandAction::ToggleRuler));
    }

    #[test]
    fn test_replaceall_valid() {
        let reg = CommandRegistry::new();
        match reg.execute("replaceall foo bar") {
            CommandAction::ReplaceAll {
                pattern,
                replacement,
            } => {
                assert_eq!(pattern, "foo");
                assert_eq!(replacement, "bar");
            }
            _ => panic!("expected ReplaceAll"),
        }
    }

    #[test]
    fn test_replaceall_two_unquoted_args() {
        let reg = CommandRegistry::new();
        match reg.execute("replaceall foo bar baz") {
            CommandAction::ReplaceAll {
                pattern,
                replacement,
            } => {
                assert_eq!(pattern, "foo");
                assert_eq!(replacement, "bar");
            }
            _ => panic!("expected ReplaceAll"),
        }
    }

    #[test]
    fn test_replaceall_single_quoted_args() {
        let reg = CommandRegistry::new();
        match reg.execute("replaceall '  foo ' 'bar'") {
            CommandAction::ReplaceAll {
                pattern,
                replacement,
            } => {
                assert_eq!(pattern, "  foo ");
                assert_eq!(replacement, "bar");
            }
            _ => panic!("expected ReplaceAll"),
        }
    }

    #[test]
    fn test_replaceall_double_quoted_args() {
        let reg = CommandRegistry::new();
        match reg.execute(r#"replaceall "hello world" "goodbye""#) {
            CommandAction::ReplaceAll {
                pattern,
                replacement,
            } => {
                assert_eq!(pattern, "hello world");
                assert_eq!(replacement, "goodbye");
            }
            _ => panic!("expected ReplaceAll"),
        }
    }

    #[test]
    fn test_replaceall_quoted_empty_replacement() {
        let reg = CommandRegistry::new();
        match reg.execute("replaceall 'foo' ''") {
            CommandAction::ReplaceAll {
                pattern,
                replacement,
            } => {
                assert_eq!(pattern, "foo");
                assert_eq!(replacement, "");
            }
            _ => panic!("expected ReplaceAll"),
        }
    }

    #[test]
    fn test_replaceall_missing_args() {
        let reg = CommandRegistry::new();
        match reg.execute("replaceall") {
            CommandAction::StatusMsg(msg) => assert!(msg.contains("Usage")),
            _ => panic!("expected StatusMsg"),
        }
    }

    #[test]
    fn test_replaceall_one_arg() {
        let reg = CommandRegistry::new();
        match reg.execute("replaceall foo") {
            CommandAction::StatusMsg(msg) => assert!(msg.contains("Usage")),
            _ => panic!("expected StatusMsg"),
        }
    }

    #[test]
    fn test_comment_toggle() {
        let reg = CommandRegistry::new();
        assert!(matches!(
            reg.execute("comment"),
            CommandAction::ToggleComment
        ));
    }

    #[test]
    fn test_comment_on() {
        let reg = CommandRegistry::new();
        assert!(matches!(
            reg.execute("comment on"),
            CommandAction::CommentOn
        ));
    }

    #[test]
    fn test_comment_off() {
        let reg = CommandRegistry::new();
        assert!(matches!(
            reg.execute("comment off"),
            CommandAction::CommentOff
        ));
    }

    #[test]
    fn test_comment_invalid_arg() {
        let reg = CommandRegistry::new();
        assert!(matches!(
            reg.execute("comment foo"),
            CommandAction::StatusMsg(_)
        ));
    }

    #[test]
    fn test_selectall_command() {
        let reg = CommandRegistry::new();
        assert!(matches!(reg.execute("selectall"), CommandAction::SelectAll));
    }

    #[test]
    fn test_find_command() {
        let reg = CommandRegistry::new();
        match reg.execute("find hello") {
            CommandAction::Find(p) => assert_eq!(p, "hello"),
            _ => panic!("expected Find"),
        }
    }

    #[test]
    fn test_find_quoted_pattern() {
        let reg = CommandRegistry::new();
        match reg.execute("find 'hello world'") {
            CommandAction::Find(p) => assert_eq!(p, "hello world"),
            _ => panic!("expected Find"),
        }
    }

    #[test]
    fn test_find_no_args() {
        let reg = CommandRegistry::new();
        match reg.execute("find") {
            CommandAction::StatusMsg(msg) => assert!(msg.contains("Usage")),
            _ => panic!("expected StatusMsg"),
        }
    }

    #[test]
    fn test_parse_args_basic() {
        assert_eq!(parse_args("foo bar"), vec!["foo", "bar"]);
    }

    #[test]
    fn test_parse_args_single_quotes() {
        assert_eq!(parse_args("'foo bar' baz"), vec!["foo bar", "baz"]);
    }

    #[test]
    fn test_parse_args_double_quotes() {
        assert_eq!(parse_args(r#""foo bar" baz"#), vec!["foo bar", "baz"]);
    }

    #[test]
    fn test_parse_args_empty_quoted() {
        assert_eq!(parse_args("'' 'x'"), vec!["", "x"]);
    }

    #[test]
    fn test_parse_args_extra_whitespace() {
        assert_eq!(parse_args("  foo   bar  "), vec!["foo", "bar"]);
    }

    #[test]
    fn test_parse_args_empty() {
        let result: Vec<String> = Vec::new();
        assert_eq!(parse_args(""), result);
        assert_eq!(parse_args("   "), result);
    }

    #[test]
    fn test_unknown_command() {
        let reg = CommandRegistry::new();
        match reg.execute("foobar") {
            CommandAction::StatusMsg(msg) => assert!(msg.contains("Unknown command: foobar")),
            _ => panic!("expected StatusMsg"),
        }
    }

    #[test]
    fn test_whitespace_handling() {
        let reg = CommandRegistry::new();
        assert!(matches!(reg.execute("  save  "), CommandAction::Save));
    }

    #[test]
    fn test_command_names() {
        let reg = CommandRegistry::new();
        let names = reg.command_names();
        assert!(names.contains(&"save"));
        assert!(names.contains(&"quit"));
        assert!(names.contains(&"q"));
        assert!(names.contains(&"find"));
        assert!(names.contains(&"goto"));
        assert!(names.contains(&"ruler"));
        assert!(names.contains(&"replaceall"));
        assert!(names.contains(&"comment"));
        assert!(names.contains(&"selectall"));
        assert!(names.contains(&"trim"));
        // Should be sorted
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
