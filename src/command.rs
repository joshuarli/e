/// Command registry: maps command names to functions.
use std::collections::HashMap;

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
    StatusMsg(String),
}

pub struct CommandRegistry {
    commands: HashMap<String, CommandFn>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut commands: HashMap<String, CommandFn> = HashMap::new();

        commands.insert("save".to_string(), cmd_save);
        commands.insert("quit".to_string(), cmd_quit);
        commands.insert("q".to_string(), cmd_quit);
        commands.insert("goto".to_string(), cmd_goto);
        commands.insert("ruler".to_string(), cmd_ruler);
        commands.insert("replaceall".to_string(), cmd_replaceall);
        commands.insert("comment".to_string(), cmd_comment);

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

fn cmd_replaceall(args: &str, ctx: &mut CommandContext) {
    // replaceall <regex> <replacement>
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 {
        ctx.action =
            CommandAction::StatusMsg("Usage: replaceall <regex> <replacement>".to_string());
        return;
    }
    ctx.action = CommandAction::ReplaceAll {
        pattern: parts[0].to_string(),
        replacement: parts[1].to_string(),
    };
}

fn cmd_comment(_args: &str, ctx: &mut CommandContext) {
    ctx.action = CommandAction::ToggleComment;
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
    fn test_replaceall_replacement_with_spaces() {
        let reg = CommandRegistry::new();
        match reg.execute("replaceall foo bar baz") {
            CommandAction::ReplaceAll {
                pattern,
                replacement,
            } => {
                assert_eq!(pattern, "foo");
                assert_eq!(replacement, "bar baz");
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
}
