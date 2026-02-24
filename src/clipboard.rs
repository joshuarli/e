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
}

impl Clipboard {
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
        }
    }

    pub fn copy(&mut self, text: &str) {
        self.internal = text.to_string();
        let _ = match &self.backend {
            ClipboardBackend::Pbcopy => pipe_to_command("pbcopy", &[], text),
            ClipboardBackend::WlCopy => pipe_to_command("wl-copy", &[], text),
            ClipboardBackend::Xclip => pipe_to_command("xclip", &["-selection", "clipboard"], text),
            ClipboardBackend::Xsel => pipe_to_command("xsel", &["--clipboard", "--input"], text),
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
        }
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
}
