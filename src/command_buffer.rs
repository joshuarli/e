/// Modal mini-editor for command palette, find, goto, save-as prompt.
pub struct CommandBuffer {
    pub input: String,
    pub cursor: usize,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
    pub prompt: String,
    pub mode: CommandBufferMode,
    pub active: bool,
    /// Completion suggestions displayed below the command line.
    pub completions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommandBufferMode {
    Command,  // ^p command palette
    Find,     // ^f regex find
    Goto,     // ^l goto line
    Prompt,   // save-as, quit confirmation, etc.
    SudoSave, // password prompt for sudo save
}

pub enum CommandBufferResult {
    /// User pressed Enter — submit the input.
    Submit(String),
    /// User pressed Escape — cancel.
    Cancel,
    /// Still editing.
    Continue,
    /// Input changed (for live find).
    Changed(String),
    /// User pressed Tab — request completion.
    TabComplete,
}

impl CommandBuffer {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_idx: None,
            prompt: "> ".to_string(),
            mode: CommandBufferMode::Command,
            active: false,
            completions: Vec::new(),
        }
    }

    pub fn open(&mut self, mode: CommandBufferMode, prompt: &str, prefill: &str) {
        self.mode = mode;
        self.prompt = prompt.to_string();
        self.input = prefill.to_string();
        self.cursor = self.input.len();
        self.active = true;
        self.history_idx = None;
    }

    pub fn close(&mut self) {
        if !self.input.is_empty() {
            self.history.push(self.input.clone());
        }
        self.input.clear();
        self.cursor = 0;
        self.active = false;
        self.history_idx = None;
        self.completions.clear();
    }

    pub fn display_line(&self) -> String {
        if self.mode == CommandBufferMode::SudoSave {
            let masked: String = "*".repeat(self.input.len());
            format!("{}{}", self.prompt, masked)
        } else {
            format!("{}{}", self.prompt, self.input)
        }
    }

    pub fn handle_key(&mut self, key: termion::event::Key) -> CommandBufferResult {
        use termion::event::Key;
        match key {
            Key::Char('\n') => {
                let val = self.input.clone();
                CommandBufferResult::Submit(val)
            }
            Key::Esc | Key::Ctrl('q') => CommandBufferResult::Cancel,
            Key::Char('\t') => {
                self.completions.clear();
                CommandBufferResult::TabComplete
            }
            Key::Char(c) => {
                self.completions.clear();
                self.input.insert(self.cursor, c);
                self.cursor += 1;
                CommandBufferResult::Changed(self.input.clone())
            }
            Key::Backspace => {
                self.completions.clear();
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.input.remove(self.cursor);
                    CommandBufferResult::Changed(self.input.clone())
                } else {
                    CommandBufferResult::Continue
                }
            }
            Key::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                CommandBufferResult::Continue
            }
            Key::Right => {
                if self.cursor < self.input.len() {
                    self.cursor += 1;
                }
                CommandBufferResult::Continue
            }
            Key::Up => {
                self.history_prev();
                CommandBufferResult::Continue
            }
            Key::Down => {
                self.history_next();
                CommandBufferResult::Continue
            }
            _ => CommandBufferResult::Continue,
        }
    }

    pub fn insert_str(&mut self, s: &str) -> CommandBufferResult {
        self.completions.clear();
        let clean: String = s.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        self.input.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
        CommandBufferResult::Changed(self.input.clone())
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            Some(0) => return,
            Some(i) => i - 1,
            None => self.history.len() - 1,
        };
        self.history_idx = Some(idx);
        self.input = self.history[idx].clone();
        self.cursor = self.input.len();
    }

    fn history_next(&mut self) {
        if let Some(idx) = self.history_idx {
            if idx + 1 < self.history.len() {
                let new_idx = idx + 1;
                self.history_idx = Some(new_idx);
                self.input = self.history[new_idx].clone();
                self.cursor = self.input.len();
            } else {
                self.history_idx = None;
                self.input.clear();
                self.cursor = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termion::event::Key;

    #[test]
    fn test_new_is_inactive() {
        let cb = CommandBuffer::new();
        assert!(!cb.active);
        assert!(cb.input.is_empty());
        assert_eq!(cb.cursor, 0);
    }

    #[test]
    fn test_open_activates() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "");
        assert!(cb.active);
        assert_eq!(cb.mode, CommandBufferMode::Command);
        assert_eq!(cb.prompt, "> ");
    }

    #[test]
    fn test_open_with_prefill() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Goto, "goto: ", "42");
        assert_eq!(cb.input, "42");
        assert_eq!(cb.cursor, 2);
    }

    #[test]
    fn test_close_deactivates() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "hello");
        cb.close();
        assert!(!cb.active);
        assert!(cb.input.is_empty());
        assert_eq!(cb.cursor, 0);
    }

    #[test]
    fn test_close_saves_to_history() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "");
        cb.handle_key(Key::Char('a'));
        cb.handle_key(Key::Char('b'));
        cb.close();
        assert_eq!(cb.history, vec!["ab"]);
    }

    #[test]
    fn test_close_empty_doesnt_save_history() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "");
        cb.close();
        assert!(cb.history.is_empty());
    }

    #[test]
    fn test_display_line() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Find, "find: ", "hello");
        assert_eq!(cb.display_line(), "find: hello");
    }

    #[test]
    fn test_char_input() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "");

        match cb.handle_key(Key::Char('a')) {
            CommandBufferResult::Changed(s) => assert_eq!(s, "a"),
            _ => panic!("expected Changed"),
        }
        assert_eq!(cb.input, "a");
        assert_eq!(cb.cursor, 1);

        match cb.handle_key(Key::Char('b')) {
            CommandBufferResult::Changed(s) => assert_eq!(s, "ab"),
            _ => panic!("expected Changed"),
        }
    }

    #[test]
    fn test_enter_submits() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "test");
        match cb.handle_key(Key::Char('\n')) {
            CommandBufferResult::Submit(s) => assert_eq!(s, "test"),
            _ => panic!("expected Submit"),
        }
    }

    #[test]
    fn test_escape_cancels() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "test");
        assert!(matches!(
            cb.handle_key(Key::Esc),
            CommandBufferResult::Cancel
        ));
    }

    #[test]
    fn test_backspace() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "abc");
        match cb.handle_key(Key::Backspace) {
            CommandBufferResult::Changed(s) => assert_eq!(s, "ab"),
            _ => panic!("expected Changed"),
        }
        assert_eq!(cb.cursor, 2);
    }

    #[test]
    fn test_backspace_at_start() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "");
        assert!(matches!(
            cb.handle_key(Key::Backspace),
            CommandBufferResult::Continue
        ));
    }

    #[test]
    fn test_left_right_cursor() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "abc");
        assert_eq!(cb.cursor, 3);

        cb.handle_key(Key::Left);
        assert_eq!(cb.cursor, 2);

        cb.handle_key(Key::Left);
        assert_eq!(cb.cursor, 1);

        cb.handle_key(Key::Right);
        assert_eq!(cb.cursor, 2);
    }

    #[test]
    fn test_left_at_start() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "a");
        cb.handle_key(Key::Left);
        assert_eq!(cb.cursor, 0);
        cb.handle_key(Key::Left); // shouldn't go negative
        assert_eq!(cb.cursor, 0);
    }

    #[test]
    fn test_right_at_end() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "ab");
        assert_eq!(cb.cursor, 2);
        cb.handle_key(Key::Right);
        assert_eq!(cb.cursor, 2); // shouldn't go past end
    }

    #[test]
    fn test_insert_in_middle() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "ac");
        cb.handle_key(Key::Left); // cursor at 1
        cb.handle_key(Key::Char('b'));
        assert_eq!(cb.input, "abc");
        assert_eq!(cb.cursor, 2);
    }

    #[test]
    fn test_history_navigation() {
        let mut cb = CommandBuffer::new();
        // Simulate previous sessions
        cb.history = vec!["first".to_string(), "second".to_string()];

        cb.open(CommandBufferMode::Command, "> ", "");

        cb.handle_key(Key::Up); // should show "second"
        assert_eq!(cb.input, "second");

        cb.handle_key(Key::Up); // should show "first"
        assert_eq!(cb.input, "first");

        cb.handle_key(Key::Up); // at beginning, shouldn't change
        assert_eq!(cb.input, "first");

        cb.handle_key(Key::Down); // back to "second"
        assert_eq!(cb.input, "second");

        cb.handle_key(Key::Down); // past end, clears
        assert_eq!(cb.input, "");
    }

    #[test]
    fn test_history_empty() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "");
        // Up with no history should do nothing
        cb.handle_key(Key::Up);
        assert_eq!(cb.input, "");
    }

    #[test]
    fn test_unknown_key_continues() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "");
        assert!(matches!(
            cb.handle_key(Key::F(1)),
            CommandBufferResult::Continue
        ));
    }

    #[test]
    fn test_sudo_save_display_masked() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::SudoSave, "password: ", "");
        cb.handle_key(Key::Char('s'));
        cb.handle_key(Key::Char('e'));
        cb.handle_key(Key::Char('c'));
        assert_eq!(cb.display_line(), "password: ***");
    }

    #[test]
    fn test_ctrl_q_cancels() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "test");
        assert!(matches!(
            cb.handle_key(Key::Ctrl('q')),
            CommandBufferResult::Cancel
        ));
    }

    #[test]
    fn test_insert_str() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "");
        match cb.insert_str("hello\nworld") {
            CommandBufferResult::Changed(s) => assert_eq!(s, "helloworld"), // newlines filtered
            _ => panic!("expected Changed"),
        }
    }

    #[test]
    fn test_tab_requests_completion() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "sav");
        cb.completions = vec!["save".to_string()];
        match cb.handle_key(Key::Char('\t')) {
            CommandBufferResult::TabComplete => {}
            _ => panic!("expected TabComplete"),
        }
        // Tab clears completions
        assert!(cb.completions.is_empty());
    }

    #[test]
    fn test_history_down_no_idx_noop() {
        let mut cb = CommandBuffer::new();
        cb.history = vec!["first".to_string()];
        cb.open(CommandBufferMode::Command, "> ", "");
        // Down without having navigated up should do nothing
        cb.handle_key(Key::Down);
        assert_eq!(cb.input, "");
    }

    #[test]
    fn test_close_clears_completions() {
        let mut cb = CommandBuffer::new();
        cb.open(CommandBufferMode::Command, "> ", "test");
        cb.completions = vec!["comp1".to_string()];
        cb.close();
        assert!(cb.completions.is_empty());
    }
}
