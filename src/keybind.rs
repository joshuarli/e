/// Keybinding configuration: INI parser + key table.
///
/// Config file: `~/.config/e/keybindings.ini`
/// Format:
///   [keybindings]
///   ctrl+s = save
///   ctrl+q = quit
///   ctrl+f = find
///   ...
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use termion::event::Key;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EditorAction {
    Save,
    Quit,
    Undo,
    Redo,
    SelectAll,
    Copy,
    Cut,
    Paste,
    KillLine,
    GotoTop,
    GotoEnd,
    ToggleRuler,
    CommandPalette,
    GotoLine,
    Find,
    CtrlBackspace,
    ToggleComment,
    DuplicateLine,
    SelectWord,
}

pub struct KeybindingTable {
    bindings: HashMap<KeyCombo, EditorAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct KeyCombo {
    key: Key,
}

impl KeybindingTable {
    pub fn with_defaults() -> Self {
        let mut bindings = HashMap::new();
        bindings.insert(kc(Key::Ctrl('s')), EditorAction::Save);
        bindings.insert(kc(Key::Ctrl('q')), EditorAction::Quit);
        bindings.insert(kc(Key::Ctrl('z')), EditorAction::Undo);
        bindings.insert(kc(Key::Ctrl('y')), EditorAction::Redo);
        bindings.insert(kc(Key::Ctrl('a')), EditorAction::SelectAll);
        bindings.insert(kc(Key::Ctrl('c')), EditorAction::Copy);
        bindings.insert(kc(Key::Ctrl('x')), EditorAction::Cut);
        bindings.insert(kc(Key::Ctrl('v')), EditorAction::Paste);
        bindings.insert(kc(Key::Ctrl('k')), EditorAction::KillLine);
        bindings.insert(kc(Key::Ctrl('t')), EditorAction::GotoTop);
        bindings.insert(kc(Key::Ctrl('g')), EditorAction::GotoEnd);
        bindings.insert(kc(Key::Ctrl('r')), EditorAction::ToggleRuler);
        bindings.insert(kc(Key::Ctrl('p')), EditorAction::CommandPalette);
        bindings.insert(kc(Key::Ctrl('l')), EditorAction::GotoLine);
        bindings.insert(kc(Key::Ctrl('f')), EditorAction::Find);
        bindings.insert(kc(Key::Ctrl('h')), EditorAction::CtrlBackspace);
        bindings.insert(kc(Key::Ctrl('d')), EditorAction::ToggleComment);
        bindings.insert(kc(Key::Ctrl('j')), EditorAction::DuplicateLine);
        bindings.insert(kc(Key::Ctrl('w')), EditorAction::SelectWord);
        Self { bindings }
    }

    pub fn lookup(&self, key: Key) -> Option<&EditorAction> {
        self.bindings.get(&KeyCombo { key })
    }

    /// Try to load overrides from `~/.config/e/keybindings.ini`.
    pub fn load_config(&mut self) {
        let path = config_path();
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return,
        };

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            if let Some((key_str, action_str)) = line.split_once('=') {
                let key_str = key_str.trim().to_lowercase();
                let action_str = action_str.trim().to_lowercase();

                if let (Some(key), Some(action)) = (parse_key(&key_str), parse_action(&action_str))
                {
                    self.bindings.insert(KeyCombo { key }, action);
                }
            }
        }
    }
}

fn kc(key: Key) -> KeyCombo {
    KeyCombo { key }
}

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("e")
        .join("keybindings.ini")
}

pub fn parse_key(s: &str) -> Option<Key> {
    if let Some(ch) = s.strip_prefix("ctrl+")
        && ch.len() == 1
    {
        return Some(Key::Ctrl(ch.chars().next()?));
    }
    None
}

pub fn parse_action(s: &str) -> Option<EditorAction> {
    match s {
        "save" => Some(EditorAction::Save),
        "quit" => Some(EditorAction::Quit),
        "undo" => Some(EditorAction::Undo),
        "redo" => Some(EditorAction::Redo),
        "selectall" => Some(EditorAction::SelectAll),
        "copy" => Some(EditorAction::Copy),
        "cut" => Some(EditorAction::Cut),
        "paste" => Some(EditorAction::Paste),
        "killline" => Some(EditorAction::KillLine),
        "gototop" => Some(EditorAction::GotoTop),
        "gotoend" => Some(EditorAction::GotoEnd),
        "toggleruler" => Some(EditorAction::ToggleRuler),
        "commandpalette" => Some(EditorAction::CommandPalette),
        "gotoline" => Some(EditorAction::GotoLine),
        "find" => Some(EditorAction::Find),
        "ctrlbackspace" => Some(EditorAction::CtrlBackspace),
        "togglecomment" => Some(EditorAction::ToggleComment),
        "duplicateline" => Some(EditorAction::DuplicateLine),
        "selectword" => Some(EditorAction::SelectWord),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termion::event::Key;

    // -- default bindings -----------------------------------------------------

    #[test]
    fn test_defaults_has_save() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(kb.lookup(Key::Ctrl('s')), Some(&EditorAction::Save));
    }

    #[test]
    fn test_defaults_has_quit() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(kb.lookup(Key::Ctrl('q')), Some(&EditorAction::Quit));
    }

    #[test]
    fn test_defaults_has_undo_redo() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(kb.lookup(Key::Ctrl('z')), Some(&EditorAction::Undo));
        assert_eq!(kb.lookup(Key::Ctrl('y')), Some(&EditorAction::Redo));
    }

    #[test]
    fn test_defaults_has_clipboard() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(kb.lookup(Key::Ctrl('c')), Some(&EditorAction::Copy));
        assert_eq!(kb.lookup(Key::Ctrl('x')), Some(&EditorAction::Cut));
        assert_eq!(kb.lookup(Key::Ctrl('v')), Some(&EditorAction::Paste));
    }

    #[test]
    fn test_defaults_has_navigation() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(kb.lookup(Key::Ctrl('t')), Some(&EditorAction::GotoTop));
        assert_eq!(kb.lookup(Key::Ctrl('g')), Some(&EditorAction::GotoEnd));
        assert_eq!(kb.lookup(Key::Ctrl('l')), Some(&EditorAction::GotoLine));
    }

    #[test]
    fn test_defaults_has_find() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(kb.lookup(Key::Ctrl('f')), Some(&EditorAction::Find));
    }

    #[test]
    fn test_defaults_has_command_palette() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(
            kb.lookup(Key::Ctrl('p')),
            Some(&EditorAction::CommandPalette)
        );
    }

    #[test]
    fn test_defaults_has_misc() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(kb.lookup(Key::Ctrl('a')), Some(&EditorAction::SelectAll));
        assert_eq!(kb.lookup(Key::Ctrl('k')), Some(&EditorAction::KillLine));
        assert_eq!(kb.lookup(Key::Ctrl('r')), Some(&EditorAction::ToggleRuler));
        assert_eq!(
            kb.lookup(Key::Ctrl('h')),
            Some(&EditorAction::CtrlBackspace)
        );
    }

    #[test]
    fn test_defaults_has_toggle_comment() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(
            kb.lookup(Key::Ctrl('d')),
            Some(&EditorAction::ToggleComment)
        );
    }

    #[test]
    fn test_defaults_has_duplicate_and_selectword() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(
            kb.lookup(Key::Ctrl('j')),
            Some(&EditorAction::DuplicateLine)
        );
        assert_eq!(kb.lookup(Key::Ctrl('w')), Some(&EditorAction::SelectWord));
    }

    #[test]
    fn test_lookup_unbound_key() {
        let kb = KeybindingTable::with_defaults();
        assert_eq!(kb.lookup(Key::Char('a')), None);
        assert_eq!(kb.lookup(Key::F(1)), None);
    }

    // -- parse_key ------------------------------------------------------------

    #[test]
    fn test_parse_key_ctrl() {
        assert_eq!(parse_key("ctrl+s"), Some(Key::Ctrl('s')));
        assert_eq!(parse_key("ctrl+a"), Some(Key::Ctrl('a')));
        assert_eq!(parse_key("ctrl+z"), Some(Key::Ctrl('z')));
    }

    #[test]
    fn test_parse_key_no_prefix() {
        assert_eq!(parse_key("s"), None);
        assert_eq!(parse_key("enter"), None);
    }

    #[test]
    fn test_parse_key_ctrl_multichar() {
        // ctrl+ab is not a single char
        assert_eq!(parse_key("ctrl+ab"), None);
    }

    #[test]
    fn test_parse_key_empty_after_ctrl() {
        assert_eq!(parse_key("ctrl+"), None);
    }

    #[test]
    fn test_parse_key_no_ctrl_prefix() {
        assert_eq!(parse_key("alt+s"), None);
        assert_eq!(parse_key("shift+s"), None);
    }

    // -- parse_action ---------------------------------------------------------

    #[test]
    fn test_parse_action_all_valid() {
        assert_eq!(parse_action("save"), Some(EditorAction::Save));
        assert_eq!(parse_action("quit"), Some(EditorAction::Quit));
        assert_eq!(parse_action("undo"), Some(EditorAction::Undo));
        assert_eq!(parse_action("redo"), Some(EditorAction::Redo));
        assert_eq!(parse_action("selectall"), Some(EditorAction::SelectAll));
        assert_eq!(parse_action("copy"), Some(EditorAction::Copy));
        assert_eq!(parse_action("cut"), Some(EditorAction::Cut));
        assert_eq!(parse_action("paste"), Some(EditorAction::Paste));
        assert_eq!(parse_action("killline"), Some(EditorAction::KillLine));
        assert_eq!(parse_action("gototop"), Some(EditorAction::GotoTop));
        assert_eq!(parse_action("gotoend"), Some(EditorAction::GotoEnd));
        assert_eq!(parse_action("toggleruler"), Some(EditorAction::ToggleRuler));
        assert_eq!(
            parse_action("commandpalette"),
            Some(EditorAction::CommandPalette)
        );
        assert_eq!(parse_action("gotoline"), Some(EditorAction::GotoLine));
        assert_eq!(parse_action("find"), Some(EditorAction::Find));
        assert_eq!(
            parse_action("ctrlbackspace"),
            Some(EditorAction::CtrlBackspace)
        );
        assert_eq!(
            parse_action("togglecomment"),
            Some(EditorAction::ToggleComment)
        );
        assert_eq!(
            parse_action("duplicateline"),
            Some(EditorAction::DuplicateLine)
        );
        assert_eq!(parse_action("selectword"), Some(EditorAction::SelectWord));
    }

    #[test]
    fn test_parse_action_unknown() {
        assert_eq!(parse_action("foobar"), None);
        assert_eq!(parse_action(""), None);
        assert_eq!(parse_action("SAVE"), None); // case-sensitive
    }

    // -- config_path ----------------------------------------------------------

    #[test]
    fn test_config_path_contains_expected_segments() {
        let path = config_path();
        let s = path.to_string_lossy();
        assert!(s.contains(".config"));
        assert!(s.contains("keybindings.ini"));
    }

    // -- load_config with INI content -----------------------------------------

    #[test]
    fn test_load_config_missing_file() {
        // Should not panic when config file doesn't exist
        let mut kb = KeybindingTable::with_defaults();
        kb.load_config();
        // Defaults should still be present
        assert_eq!(kb.lookup(Key::Ctrl('s')), Some(&EditorAction::Save));
    }

    // -- KeyCombo equality ----------------------------------------------------

    #[test]
    fn test_key_combo_equality() {
        let a = kc(Key::Ctrl('s'));
        let b = kc(Key::Ctrl('s'));
        assert_eq!(a, b);
    }

    #[test]
    fn test_key_combo_inequality() {
        let a = kc(Key::Ctrl('s'));
        let b = kc(Key::Ctrl('q'));
        assert_ne!(a, b);
    }

    #[test]
    fn test_load_config_from_file() {
        let dir = std::env::temp_dir().join("e_test_keybind");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("keybindings.ini");
        std::fs::write(
            &path,
            b"[keybindings]\nctrl+s = quit\n# comment line\n\n[other]\nctrl+z = save\n",
        )
        .unwrap();

        let mut kb = KeybindingTable::with_defaults();
        // Manually parse the file content (since load_config reads from fixed path)
        let content = std::fs::read_to_string(&path).unwrap();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            if let Some((key_str, action_str)) = line.split_once('=') {
                let key_str = key_str.trim().to_lowercase();
                let action_str = action_str.trim().to_lowercase();
                if let (Some(key), Some(action)) = (parse_key(&key_str), parse_action(&action_str))
                {
                    kb.bindings.insert(KeyCombo { key }, action);
                }
            }
        }

        // ctrl+s should now be Quit instead of Save
        assert_eq!(kb.lookup(Key::Ctrl('s')), Some(&EditorAction::Quit));
        // ctrl+z should now be Save instead of Undo
        assert_eq!(kb.lookup(Key::Ctrl('z')), Some(&EditorAction::Save));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
