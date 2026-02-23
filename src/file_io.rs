use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Read a file, converting CRLF → LF. Returns the bytes.
pub fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    let data = fs::read(path)?;
    Ok(normalize_line_endings(&data))
}

/// Strip CRLF → LF.
pub fn normalize_line_endings(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
            out.push(b'\n');
            i += 2;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// Write `data` to `path`. Strips trailing whitespace from each line and ensures
/// a trailing newline.
pub fn write_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let cleaned = strip_trailing_whitespace_and_ensure_newline(data);
    let mut f = fs::File::create(path)?;
    f.write_all(&cleaned)?;
    f.flush()?;
    Ok(())
}

/// Clean data for writing: strip trailing whitespace and ensure trailing newline.
pub fn clean_for_write(data: &[u8]) -> Vec<u8> {
    strip_trailing_whitespace_and_ensure_newline(data)
}

fn strip_trailing_whitespace_and_ensure_newline(data: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(data);
    let mut out = String::new();
    for line in text.split('\n') {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.into_bytes()
}

/// Directory for lock files and future buffer backups: `~/.config/e/buffers/`
fn buffers_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config/e/buffers")
}

/// Encode an absolute file path for use as a filename.
/// `/path/to/file.txt` → `%2Fpath%2Fto%2Ffile.txt`
fn encode_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'/' => out.push_str("%2F"),
            b'%' => out.push_str("%25"),
            _ => out.push(b as char),
        }
    }
    out
}

/// Compute lock file path: `~/.config/e/buffers/<encoded_path>.elock`
pub fn lock_path(path: &Path) -> PathBuf {
    buffers_dir().join(format!("{}.elock", encode_path(path)))
}

/// Resolve a path to absolute. Uses canonicalize if the file exists,
/// otherwise canonicalizes the parent and appends the filename.
fn resolve_absolute(path: &Path) -> PathBuf {
    if let Ok(abs) = path.canonicalize() {
        return abs;
    }
    // File doesn't exist yet — resolve parent
    let parent = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default();
    let abs_parent = parent.canonicalize().unwrap_or_else(|_| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(parent)
    });
    abs_parent.join(name)
}

/// Acquire a lock file. Returns Err if lock already exists.
pub fn acquire_lock(path: &Path) -> Result<(), String> {
    let abs = resolve_absolute(path);
    let lock = lock_path(&abs);
    if lock.exists() {
        return Err(format!(
            "Lock file exists: {} (another e instance may be editing this file)",
            lock.display()
        ));
    }
    let dir = buffers_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create buffers dir: {}", e))?;
    fs::File::create(&lock).map_err(|e| format!("Failed to create lock file: {}", e))?;
    Ok(())
}

/// Release the lock file, ignoring errors.
pub fn release_lock(path: &Path) {
    let abs = resolve_absolute(path);
    let lock = lock_path(&abs);
    let _ = fs::remove_file(lock);
}

/// Check the first 8KB for null bytes → likely binary.
pub fn is_likely_binary(data: &[u8]) -> bool {
    let check_len = data.len().min(8192);
    data[..check_len].contains(&0)
}

/// File size in bytes.
pub fn file_size(path: &Path) -> io::Result<u64> {
    Ok(fs::metadata(path)?.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- normalize_line_endings ---------------------------------------------

    #[test]
    fn test_normalize_no_crlf() {
        assert_eq!(normalize_line_endings(b"hello\nworld"), b"hello\nworld");
    }

    #[test]
    fn test_normalize_crlf() {
        assert_eq!(
            normalize_line_endings(b"hello\r\nworld\r\n"),
            b"hello\nworld\n"
        );
    }

    #[test]
    fn test_normalize_mixed() {
        assert_eq!(normalize_line_endings(b"a\r\nb\nc\r\n"), b"a\nb\nc\n");
    }

    #[test]
    fn test_normalize_lone_cr() {
        // \r not followed by \n should be kept
        assert_eq!(normalize_line_endings(b"a\rb"), b"a\rb");
    }

    #[test]
    fn test_normalize_empty() {
        assert_eq!(normalize_line_endings(b""), b"");
    }

    #[test]
    fn test_normalize_only_crlf() {
        assert_eq!(normalize_line_endings(b"\r\n"), b"\n");
    }

    // -- strip_trailing_whitespace_and_ensure_newline -----------------------

    #[test]
    fn test_strip_trailing_whitespace() {
        let result = strip_trailing_whitespace_and_ensure_newline(b"hello   \nworld  \n");
        assert_eq!(result, b"hello\nworld\n");
    }

    #[test]
    fn test_ensure_trailing_newline() {
        let result = strip_trailing_whitespace_and_ensure_newline(b"hello");
        assert_eq!(result, b"hello\n");
    }

    #[test]
    fn test_already_has_trailing_newline() {
        let result = strip_trailing_whitespace_and_ensure_newline(b"hello\n");
        assert_eq!(result, b"hello\n");
    }

    #[test]
    fn test_strip_tabs_at_end() {
        let result = strip_trailing_whitespace_and_ensure_newline(b"hello\t\t\n");
        assert_eq!(result, b"hello\n");
    }

    #[test]
    fn test_preserves_leading_whitespace() {
        let result = strip_trailing_whitespace_and_ensure_newline(b"  hello\n");
        assert_eq!(result, b"  hello\n");
    }

    #[test]
    fn test_empty_lines_preserved() {
        let result = strip_trailing_whitespace_and_ensure_newline(b"a\n\nb\n");
        assert_eq!(result, b"a\n\nb\n");
    }

    #[test]
    fn test_only_whitespace_lines() {
        // All-whitespace lines get trimmed to empty; the function produces a single trailing newline
        let result = strip_trailing_whitespace_and_ensure_newline(b"   \n  \n");
        assert_eq!(result, b"\n");
    }

    // -- is_likely_binary ---------------------------------------------------

    #[test]
    fn test_text_not_binary() {
        assert!(!is_likely_binary(b"hello world\nfoo bar\n"));
    }

    #[test]
    fn test_binary_with_null() {
        assert!(is_likely_binary(b"hello\x00world"));
    }

    #[test]
    fn test_binary_null_at_start() {
        assert!(is_likely_binary(b"\x00hello"));
    }

    #[test]
    fn test_empty_not_binary() {
        assert!(!is_likely_binary(b""));
    }

    #[test]
    fn test_binary_null_past_8kb_not_detected() {
        let mut data = vec![b'a'; 8193];
        data[8192] = 0; // null after 8KB
        assert!(!is_likely_binary(&data));
    }

    #[test]
    fn test_binary_null_within_8kb() {
        let mut data = vec![b'a'; 8192];
        data[8191] = 0; // null at end of 8KB check range
        assert!(is_likely_binary(&data));
    }

    // -- read_file / write_file (integration) -------------------------------

    #[test]
    fn test_read_write_roundtrip() {
        let dir = std::env::temp_dir().join("e_test_roundtrip");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");

        let data = b"hello\nworld\n";
        write_file(&path, data).unwrap();

        let read_back = read_file(&path).unwrap();
        assert_eq!(read_back, b"hello\nworld\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_strips_trailing_whitespace() {
        let dir = std::env::temp_dir().join("e_test_strip");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");

        write_file(&path, b"hello   \nworld  ").unwrap();
        let read_back = fs::read(&path).unwrap();
        assert_eq!(read_back, b"hello\nworld\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_normalizes_crlf_on_read() {
        let dir = std::env::temp_dir().join("e_test_crlf");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");

        // Write raw CRLF
        fs::write(&path, b"hello\r\nworld\r\n").unwrap();
        let data = read_file(&path).unwrap();
        assert_eq!(data, b"hello\nworld\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_file_size() {
        let dir = std::env::temp_dir().join("e_test_size");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");

        fs::write(&path, b"12345").unwrap();
        assert_eq!(file_size(&path).unwrap(), 5);

        let _ = fs::remove_dir_all(&dir);
    }
}
