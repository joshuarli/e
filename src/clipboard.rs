use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug)]
enum ClipboardBackend {
    Pbcopy,   // macOS
    WlCopy,   // Wayland
    Xclip,    // X11
    Xsel,     // X11 fallback
    Internal, // no system clipboard available
}

pub struct Clipboard {
    backend: ClipboardBackend,
    internal: String,
    internal_fragments: Option<Vec<String>>,
}

pub struct ClipboardPaste {
    pub text: String,
    pub fragments: Option<Vec<String>>,
}

impl Clipboard {
    fn fragments_for_text(&self, text: &str) -> Option<Vec<String>> {
        if text == self.internal {
            self.internal_fragments.clone()
        } else {
            None
        }
    }

    fn paste_contents_for_text(&self, text: String) -> ClipboardPaste {
        ClipboardPaste {
            fragments: self.fragments_for_text(&text),
            text,
        }
    }

    pub fn detect() -> Self {
        let backend = if cfg!(target_os = "macos") {
            if command_exists("pbcopy") {
                ClipboardBackend::Pbcopy
            } else {
                ClipboardBackend::Internal
            }
        } else {
            // Linux
            if std::env::var("WAYLAND_DISPLAY").is_ok() && command_exists("wl-copy") {
                ClipboardBackend::WlCopy
            } else if command_exists("xclip") {
                ClipboardBackend::Xclip
            } else if command_exists("xsel") {
                ClipboardBackend::Xsel
            } else {
                ClipboardBackend::Internal
            }
        };
        Self {
            backend,
            internal: String::new(),
            internal_fragments: None,
        }
    }

    pub fn copy(&mut self, text: &str) {
        self.internal = text.to_string();
        self.internal_fragments = None;
        let _ = match &self.backend {
            ClipboardBackend::Pbcopy => pipe_to_command("pbcopy", &[], text),
            ClipboardBackend::WlCopy => pipe_to_command("wl-copy", &[], text),
            ClipboardBackend::Xclip => pipe_to_command("xclip", &["-selection", "clipboard"], text),
            ClipboardBackend::Xsel => pipe_to_command("xsel", &["--clipboard", "--input"], text),
            ClipboardBackend::Internal => Ok(()),
        };
    }

    pub fn copy_multi(&mut self, fragments: &[String]) {
        let text = fragments.join("\n");
        self.internal = text.clone();
        self.internal_fragments = Some(fragments.to_vec());
        let _ = match &self.backend {
            ClipboardBackend::Pbcopy => pipe_to_command("pbcopy", &[], &text),
            ClipboardBackend::WlCopy => pipe_to_command("wl-copy", &[], &text),
            ClipboardBackend::Xclip => {
                pipe_to_command("xclip", &["-selection", "clipboard"], &text)
            }
            ClipboardBackend::Xsel => pipe_to_command("xsel", &["--clipboard", "--input"], &text),
            ClipboardBackend::Internal => Ok(()),
        };
    }

    pub fn paste(&self) -> String {
        match &self.backend {
            ClipboardBackend::Pbcopy => read_from_command("pbpaste", &[]),
            ClipboardBackend::WlCopy => read_from_command("wl-paste", &["-n"]),
            ClipboardBackend::Xclip => {
                read_from_command("xclip", &["-selection", "clipboard", "-o"])
            }
            ClipboardBackend::Xsel => read_from_command("xsel", &["--clipboard", "--output"]),
            ClipboardBackend::Internal => self.internal.clone(),
        }
    }

    pub fn paste_contents(&self) -> ClipboardPaste {
        match &self.backend {
            ClipboardBackend::Internal => ClipboardPaste {
                text: self.internal.clone(),
                fragments: self.internal_fragments.clone(),
            },
            _ => self.paste_contents_for_text(self.paste()),
        }
    }
}

fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn pipe_to_command(cmd: &str, args: &[&str], input: &str) -> Result<(), ()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;

    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(input.as_bytes()).map_err(|_| ())?;
    }
    child.wait().map_err(|_| ())?;
    Ok(())
}

fn read_from_command(cmd: &str, args: &[&str]) -> String {
    let output = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
impl Clipboard {
    pub(crate) fn internal_only() -> Self {
        Self {
            backend: ClipboardBackend::Internal,
            internal: String::new(),
            internal_fragments: None,
        }
    }

    pub(crate) fn with_backend_for_test(backend: &str) -> Self {
        let backend = match backend {
            "pbcopy" => ClipboardBackend::Pbcopy,
            "wl-copy" => ClipboardBackend::WlCopy,
            "xclip" => ClipboardBackend::Xclip,
            "xsel" => ClipboardBackend::Xsel,
            _ => ClipboardBackend::Internal,
        };
        Self {
            backend,
            internal: String::new(),
            internal_fragments: None,
        }
    }

    pub(crate) fn paste_contents_from_text_for_test(&self, text: &str) -> ClipboardPaste {
        self.paste_contents_for_text(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_copy_paste() {
        let mut clip = Clipboard::internal_only();
        clip.copy("hello world");
        assert_eq!(clip.paste(), "hello world");
    }

    #[test]
    fn test_internal_overwrite() {
        let mut clip = Clipboard::internal_only();
        clip.copy("first");
        clip.copy("second");
        assert_eq!(clip.paste(), "second");
    }

    #[test]
    fn test_internal_empty() {
        let clip = Clipboard::internal_only();
        assert_eq!(clip.paste(), "");
    }

    #[test]
    fn test_detect_does_not_panic() {
        let _clip = Clipboard::detect();
    }

    #[test]
    fn test_internal_multiline() {
        let mut clip = Clipboard::internal_only();
        clip.copy("line1\nline2\nline3");
        assert_eq!(clip.paste(), "line1\nline2\nline3");
    }

    #[test]
    fn test_external_paste_contents_reuses_fragments_when_text_matches() {
        let mut clip = Clipboard::with_backend_for_test("pbcopy");
        clip.copy_multi(&["hello".to_string(), "world".to_string()]);
        let contents = clip.paste_contents_from_text_for_test("hello\nworld");
        assert_eq!(contents.text, "hello\nworld");
        assert_eq!(
            contents.fragments,
            Some(vec!["hello".to_string(), "world".to_string()])
        );
    }

    #[test]
    fn test_external_paste_contents_drops_fragments_when_text_differs() {
        let mut clip = Clipboard::with_backend_for_test("pbcopy");
        clip.copy_multi(&["hello".to_string(), "world".to_string()]);
        let contents = clip.paste_contents_from_text_for_test("other");
        assert_eq!(contents.text, "other");
        assert_eq!(contents.fragments, None);
    }
}
