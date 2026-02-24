use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::operation::{Operation, OperationGroup, UndoStack};
use crate::selection::Pos;

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

// -- persistent undo history ------------------------------------------------
//
// All undo histories stored in a single file: ~/.config/e/undo.bin
// Format: [magic][version][entry_count] then length-prefixed entries.
// On save, non-matching entries are copied as raw bytes (no deserialization).
// On load, entries are scanned linearly; only the matching one is deserialized.

const UNDO_MAGIC: &[u8; 4] = b"eUND";
const UNDO_VERSION: u8 = 1;
const MAX_GROUPS: u32 = 100_000;
const MAX_ENTRIES: u32 = 10_000;

fn undo_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config/e/undo.bin")
}

/// Acquire an exclusive flock on an open file. Returns false if it fails.
fn flock_exclusive(file: &fs::File) -> bool {
    use std::os::unix::io::AsRawFd;
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) == 0 }
}

/// Acquire a shared flock on an open file.
fn flock_shared(file: &fs::File) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_SH);
    }
}

/// Release flock on an open file.
fn flock_unlock(file: &fs::File) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

// Binary encoding helpers

fn write_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_i64(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn read_u8(data: &[u8], pos: &mut usize) -> Option<u8> {
    if *pos >= data.len() {
        return None;
    }
    let v = data[*pos];
    *pos += 1;
    Some(v)
}

fn read_u32(data: &[u8], pos: &mut usize) -> Option<u32> {
    if *pos + 4 > data.len() {
        return None;
    }
    let v = u32::from_le_bytes(data[*pos..*pos + 4].try_into().ok()?);
    *pos += 4;
    Some(v)
}

fn read_u64(data: &[u8], pos: &mut usize) -> Option<u64> {
    if *pos + 8 > data.len() {
        return None;
    }
    let v = u64::from_le_bytes(data[*pos..*pos + 8].try_into().ok()?);
    *pos += 8;
    Some(v)
}

fn read_i64(data: &[u8], pos: &mut usize) -> Option<i64> {
    if *pos + 8 > data.len() {
        return None;
    }
    let v = i64::from_le_bytes(data[*pos..*pos + 8].try_into().ok()?);
    *pos += 8;
    Some(v)
}

fn serialize_group(buf: &mut Vec<u8>, group: &OperationGroup) {
    write_u32(buf, group.cursor_before.line as u32);
    write_u32(buf, group.cursor_before.col as u32);
    write_u32(buf, group.cursor_after.line as u32);
    write_u32(buf, group.cursor_after.col as u32);
    write_u32(buf, group.ops.len() as u32);
    for op in &group.ops {
        match op {
            Operation::Insert { pos, data } => {
                write_u8(buf, 0);
                write_u64(buf, *pos as u64);
                write_u32(buf, data.len() as u32);
                buf.extend_from_slice(data);
            }
            Operation::Delete { pos, data } => {
                write_u8(buf, 1);
                write_u64(buf, *pos as u64);
                write_u32(buf, data.len() as u32);
                buf.extend_from_slice(data);
            }
        }
    }
}

fn deserialize_group(data: &[u8], pos: &mut usize) -> Option<OperationGroup> {
    let cb_line = read_u32(data, pos)? as usize;
    let cb_col = read_u32(data, pos)? as usize;
    let ca_line = read_u32(data, pos)? as usize;
    let ca_col = read_u32(data, pos)? as usize;
    let op_count = read_u32(data, pos)?;
    if op_count > MAX_GROUPS {
        return None;
    }
    let mut ops = Vec::with_capacity(op_count as usize);
    for _ in 0..op_count {
        let kind = read_u8(data, pos)?;
        let op_pos = read_u64(data, pos)? as usize;
        let data_len = read_u32(data, pos)? as usize;
        if *pos + data_len > data.len() {
            return None;
        }
        let op_data = data[*pos..*pos + data_len].to_vec();
        *pos += data_len;
        let op = match kind {
            0 => Operation::Insert {
                pos: op_pos,
                data: op_data,
            },
            1 => Operation::Delete {
                pos: op_pos,
                data: op_data,
            },
            _ => return None,
        };
        ops.push(op);
    }
    Some(OperationGroup {
        ops,
        cursor_before: Pos::new(cb_line, cb_col),
        cursor_after: Pos::new(ca_line, ca_col),
    })
}

fn deserialize_groups(data: &[u8], pos: &mut usize) -> Option<Vec<OperationGroup>> {
    let count = read_u32(data, pos)?;
    if count > MAX_GROUPS {
        return None;
    }
    let mut groups = Vec::with_capacity(count as usize);
    for _ in 0..count {
        groups.push(deserialize_group(data, pos)?);
    }
    Some(groups)
}

/// Peek at the path and mtime fields at the start of an entry body.
/// Returns `(path_bytes, mtime_secs, mtime_nanos)`.
fn entry_header(data: &[u8], entry_start: usize) -> Option<(&[u8], i64, u32)> {
    let mut p = entry_start;
    let path_len = read_u32(data, &mut p)? as usize;
    if p + path_len > data.len() {
        return None;
    }
    let path = &data[p..p + path_len];
    p += path_len;
    let secs = read_i64(data, &mut p)?;
    let nanos = read_u32(data, &mut p)?;
    Some((path, secs, nanos))
}

/// Check if an entry's mtime still matches the file on disk.
fn entry_mtime_valid(path: &[u8], stored_secs: i64, stored_nanos: u32) -> bool {
    let path_str = std::str::from_utf8(path).ok();
    let Some(path_str) = path_str else {
        return false;
    };
    let Ok(meta) = fs::metadata(path_str) else {
        return false; // file deleted
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    let duration = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    stored_secs == duration.as_secs() as i64 && stored_nanos == duration.subsec_nanos()
}

/// Scan entries in a db blob, collecting raw byte ranges of entries whose path
/// does NOT match `exclude_path` and whose mtime is still valid.
fn collect_kept_entries<'a>(data: &'a [u8], exclude_path: &[u8]) -> (Vec<&'a [u8]>, u32) {
    let mut kept = Vec::new();
    let header_len = 4 + 1 + 4; // magic + version + entry_count
    if data.len() < header_len || &data[0..4] != UNDO_MAGIC || data[4] != UNDO_VERSION {
        return (kept, 0);
    }
    let mut pos = 5usize;
    let count = match read_u32(data, &mut pos) {
        Some(c) => c.min(MAX_ENTRIES),
        None => return (kept, 0),
    };
    for _ in 0..count {
        let entry_len = match read_u32(data, &mut pos) {
            Some(l) => l as usize,
            None => break,
        };
        if pos + entry_len > data.len() {
            break;
        }
        let entry_body = &data[pos..pos + entry_len];
        pos += entry_len;
        if let Some((path, secs, nanos)) = entry_header(data, pos - entry_len)
            && path != exclude_path
            && entry_mtime_valid(path, secs, nanos)
        {
            kept.push(entry_body);
        }
    }
    let kept_count = kept.len() as u32;
    (kept, kept_count)
}

/// Save undo history to disk. Errors are silently ignored.
pub fn save_undo_history(file_path: &Path, undo_stack: &UndoStack) {
    let _ = save_undo_history_to(&undo_db_path(), file_path, undo_stack);
}

fn save_undo_history_to(db_path: &Path, file_path: &Path, undo_stack: &UndoStack) -> Option<()> {
    let abs_path = resolve_absolute(file_path);
    let abs_str = abs_path.to_string_lossy();
    let path_bytes = abs_str.as_bytes();

    let meta = fs::metadata(file_path).ok()?;
    let mtime = meta.modified().ok()?;
    let duration = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    // Serialize new entry body
    let mut entry = Vec::new();
    write_u32(&mut entry, path_bytes.len() as u32);
    entry.extend_from_slice(path_bytes);
    write_i64(&mut entry, duration.as_secs() as i64);
    write_u32(&mut entry, duration.subsec_nanos());
    let (undo, redo) = undo_stack.stacks();
    write_u32(&mut entry, undo.len() as u32);
    for group in undo {
        serialize_group(&mut entry, group);
    }
    write_u32(&mut entry, redo.len() as u32);
    for group in redo {
        serialize_group(&mut entry, group);
    }

    // Ensure parent dir exists, open db file, lock it
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    let lock_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(db_path)
        .ok()?;
    if !flock_exclusive(&lock_file) {
        return None;
    }

    // Read existing db under lock, keep fresh entries for other paths
    let existing = fs::read(db_path).unwrap_or_default();
    let (kept, kept_count) = collect_kept_entries(&existing, path_bytes);

    // Write new db
    let mut buf = Vec::new();
    buf.extend_from_slice(UNDO_MAGIC);
    write_u8(&mut buf, UNDO_VERSION);
    write_u32(&mut buf, kept_count + 1);
    for entry_body in &kept {
        write_u32(&mut buf, entry_body.len() as u32);
        buf.extend_from_slice(entry_body);
    }
    write_u32(&mut buf, entry.len() as u32);
    buf.extend_from_slice(&entry);

    fs::write(db_path, &buf).ok()?;
    flock_unlock(&lock_file);
    Some(())
}

/// Load undo history from disk. Errors and mtime mismatches silently ignored.
pub fn load_undo_history(file_path: &Path, undo_stack: &mut UndoStack) {
    let _ = load_undo_history_from(&undo_db_path(), file_path, undo_stack);
}

fn load_undo_history_from(
    db_path: &Path,
    file_path: &Path,
    undo_stack: &mut UndoStack,
) -> Option<()> {
    let abs_path = resolve_absolute(file_path);
    let abs_str = abs_path.to_string_lossy();
    let target_path = abs_str.as_bytes();

    // Shared lock to prevent reading a half-written file
    let lock_file = fs::File::open(db_path).ok()?;
    flock_shared(&lock_file);
    let data = fs::read(db_path).ok()?;
    if data.len() < 9 || &data[0..4] != UNDO_MAGIC {
        return None;
    }
    let mut pos = 4usize;
    let version = read_u8(&data, &mut pos)?;
    if version != UNDO_VERSION {
        return None;
    }

    let count = read_u32(&data, &mut pos)?;
    for _ in 0..count.min(MAX_ENTRIES) {
        let entry_len = read_u32(&data, &mut pos)? as usize;
        let entry_start = pos;
        if pos + entry_len > data.len() {
            return None;
        }

        // Read path
        let path_len = read_u32(&data, &mut pos)? as usize;
        if pos + path_len > data.len() {
            return None;
        }
        let entry_path = &data[pos..pos + path_len];
        pos += path_len;

        if entry_path == target_path {
            // Mtime check
            let stored_secs = read_i64(&data, &mut pos)?;
            let stored_nanos = read_u32(&data, &mut pos)?;

            let meta = fs::metadata(file_path).ok()?;
            let mtime = meta.modified().ok()?;
            let duration = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            if stored_secs != duration.as_secs() as i64 || stored_nanos != duration.subsec_nanos() {
                return None;
            }

            let undo = deserialize_groups(&data, &mut pos)?;
            let redo = deserialize_groups(&data, &mut pos)?;
            undo_stack.restore(undo, redo);
            return Some(());
        }

        // Skip this entry
        pos = entry_start + entry_len;
    }
    None
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

    // -- undo history persistence -------------------------------------------

    fn make_test_stack() -> UndoStack {
        let mut stack = UndoStack::new();
        stack.record(
            Operation::Insert {
                pos: 0,
                data: b"hello".to_vec(),
            },
            Pos::new(0, 0),
            Pos::new(0, 5),
        );
        stack.seal();
        stack.record(
            Operation::Delete {
                pos: 3,
                data: b"lo".to_vec(),
            },
            Pos::new(0, 5),
            Pos::new(0, 3),
        );
        stack.seal();
        stack
    }

    #[test]
    fn test_undo_history_roundtrip() {
        let dir = std::env::temp_dir().join("e_test_undo_rt");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("undo.bin");
        fs::write(&path, b"hel").unwrap();

        let stack = make_test_stack();
        save_undo_history_to(&db, &path, &stack);

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        let (undo, redo) = loaded.stacks();
        assert_eq!(undo.len(), 2);
        assert!(redo.is_empty());

        assert_eq!(undo[0].cursor_before, Pos::new(0, 0));
        assert_eq!(undo[0].cursor_after, Pos::new(0, 5));
        assert_eq!(undo[0].ops.len(), 1);
        assert_eq!(undo[1].cursor_before, Pos::new(0, 5));
        assert_eq!(undo[1].cursor_after, Pos::new(0, 3));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_with_redo() {
        let dir = std::env::temp_dir().join("e_test_undo_redo");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("undo.bin");
        fs::write(&path, b"hel").unwrap();

        let mut stack = make_test_stack();
        stack.undo();
        save_undo_history_to(&db, &path, &stack);

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        let (undo, redo) = loaded.stacks();
        assert_eq!(undo.len(), 1);
        assert_eq!(redo.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_mtime_mismatch() {
        let dir = std::env::temp_dir().join("e_test_undo_mtime");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("undo.bin");
        fs::write(&path, b"hello").unwrap();

        let stack = make_test_stack();
        save_undo_history_to(&db, &path, &stack);

        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&path, b"changed").unwrap();

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        let (undo, redo) = loaded.stacks();
        assert!(undo.is_empty());
        assert!(redo.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_corrupt_db() {
        let dir = std::env::temp_dir().join("e_test_undo_corrupt");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("undo.bin");
        fs::write(&path, b"hello").unwrap();
        fs::write(&db, b"garbage data here").unwrap();

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        let (undo, redo) = loaded.stacks();
        assert!(undo.is_empty());
        assert!(redo.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_empty_stacks() {
        let dir = std::env::temp_dir().join("e_test_undo_empty");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("undo.bin");
        fs::write(&path, b"hello").unwrap();

        let stack = UndoStack::new();
        save_undo_history_to(&db, &path, &stack);

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        let (undo, redo) = loaded.stacks();
        assert!(undo.is_empty());
        assert!(redo.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_bad_magic() {
        let dir = std::env::temp_dir().join("e_test_undo_magic");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("undo.bin");
        fs::write(&path, b"hello").unwrap();
        fs::write(&db, b"BADMagic").unwrap();

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        let (undo, redo) = loaded.stacks();
        assert!(undo.is_empty());
        assert!(redo.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_bad_version() {
        let dir = std::env::temp_dir().join("e_test_undo_ver");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("undo.bin");
        fs::write(&path, b"hello").unwrap();

        let mut data = Vec::new();
        data.extend_from_slice(UNDO_MAGIC);
        data.push(99);
        fs::write(&db, &data).unwrap();

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        let (undo, redo) = loaded.stacks();
        assert!(undo.is_empty());
        assert!(redo.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_multiple_files() {
        let dir = std::env::temp_dir().join("e_test_undo_multi");
        let _ = fs::create_dir_all(&dir);
        let path_a = dir.join("a.txt");
        let path_b = dir.join("b.txt");
        let db = dir.join("undo.bin");
        fs::write(&path_a, b"aaa").unwrap();
        fs::write(&path_b, b"bbb").unwrap();

        let stack_a = make_test_stack();
        save_undo_history_to(&db, &path_a, &stack_a);

        let mut stack_b = UndoStack::new();
        stack_b.record(
            Operation::Insert {
                pos: 0,
                data: b"x".to_vec(),
            },
            Pos::new(0, 0),
            Pos::new(0, 1),
        );
        stack_b.seal();
        save_undo_history_to(&db, &path_b, &stack_b);

        // Both should be independently loadable
        let mut loaded_a = UndoStack::new();
        load_undo_history_from(&db, &path_a, &mut loaded_a);
        assert_eq!(loaded_a.stacks().0.len(), 2);

        let mut loaded_b = UndoStack::new();
        load_undo_history_from(&db, &path_b, &mut loaded_b);
        assert_eq!(loaded_b.stacks().0.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_update_replaces_entry() {
        let dir = std::env::temp_dir().join("e_test_undo_replace");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("undo.bin");
        fs::write(&path, b"hello").unwrap();

        let stack = make_test_stack();
        save_undo_history_to(&db, &path, &stack);

        // Save again with different stack — should replace, not duplicate
        let mut stack2 = UndoStack::new();
        stack2.record(
            Operation::Insert {
                pos: 0,
                data: b"x".to_vec(),
            },
            Pos::new(0, 0),
            Pos::new(0, 1),
        );
        stack2.seal();
        save_undo_history_to(&db, &path, &stack2);

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        assert_eq!(loaded.stacks().0.len(), 1); // new stack, not old

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_prunes_stale_entries() {
        let dir = std::env::temp_dir().join("e_test_undo_prune");
        let _ = fs::create_dir_all(&dir);
        let path_a = dir.join("a.txt");
        let path_b = dir.join("b.txt");
        let db = dir.join("undo.bin");
        fs::write(&path_a, b"aaa").unwrap();
        fs::write(&path_b, b"bbb").unwrap();

        // Save undo for both files
        let stack = make_test_stack();
        save_undo_history_to(&db, &path_a, &stack);
        save_undo_history_to(&db, &path_b, &stack);
        let size_before = fs::metadata(&db).unwrap().len();

        // Externally modify file_a (invalidates its mtime)
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&path_a, b"modified").unwrap();

        // Save file_b again — should prune file_a's stale entry
        save_undo_history_to(&db, &path_b, &stack);
        let size_after = fs::metadata(&db).unwrap().len();
        assert!(
            size_after < size_before,
            "db should shrink after pruning stale entry"
        );

        // file_a's entry should be gone
        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path_a, &mut loaded);
        assert!(loaded.stacks().0.is_empty());

        // file_b's entry should still work
        let mut loaded_b = UndoStack::new();
        load_undo_history_from(&db, &path_b, &mut loaded_b);
        assert_eq!(loaded_b.stacks().0.len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_undo_history_prunes_deleted_file() {
        let dir = std::env::temp_dir().join("e_test_undo_deleted");
        let _ = fs::create_dir_all(&dir);
        let path_a = dir.join("a.txt");
        let path_b = dir.join("b.txt");
        let db = dir.join("undo.bin");
        fs::write(&path_a, b"aaa").unwrap();
        fs::write(&path_b, b"bbb").unwrap();

        let stack = make_test_stack();
        save_undo_history_to(&db, &path_a, &stack);
        save_undo_history_to(&db, &path_b, &stack);

        // Delete file_a
        fs::remove_file(&path_a).unwrap();

        // Saving file_b should prune the deleted file_a entry
        save_undo_history_to(&db, &path_b, &stack);

        // Re-create file_a — its old undo entry should be gone
        fs::write(&path_a, b"aaa").unwrap();
        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path_a, &mut loaded);
        assert!(loaded.stacks().0.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_acquire_release_lock() {
        let dir = std::env::temp_dir().join("e_test_lock");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        fs::write(&path, b"hello").unwrap();

        // Acquire should succeed
        assert!(acquire_lock(&path).is_ok());
        // Second acquire should fail (lock exists)
        assert!(acquire_lock(&path).is_err());
        // Release and try again
        release_lock(&path);
        assert!(acquire_lock(&path).is_ok());
        release_lock(&path);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_clean_for_write() {
        let result = clean_for_write(b"hello   \nworld  ");
        assert_eq!(result, b"hello\nworld\n");
    }

    #[test]
    fn test_clean_for_write_empty() {
        let result = clean_for_write(b"");
        assert_eq!(result, b"\n");
    }

    #[test]
    fn test_encode_path() {
        let path = std::path::Path::new("/tmp/test/file.txt");
        let encoded = encode_path(path);
        assert!(encoded.contains("%2F"));
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn test_lock_path_contains_elock() {
        let path = std::path::Path::new("/tmp/test.txt");
        let lp = lock_path(path);
        let s = lp.to_string_lossy();
        assert!(s.ends_with(".elock"));
        assert!(s.contains("buffers"));
    }

    #[test]
    fn test_encode_path_percent() {
        let path = Path::new("/tmp/test%file");
        let encoded = encode_path(path);
        assert!(encoded.contains("%25"));
    }

    #[test]
    fn test_resolve_absolute_new_file() {
        // resolve_absolute for a file that doesn't exist yet
        let dir = std::env::temp_dir().join("e_test_resolve");
        let _ = fs::create_dir_all(&dir);
        let new_file = dir.join("brand_new.txt");
        let result = resolve_absolute(&new_file);
        assert!(result.is_absolute());
        assert!(result.to_string_lossy().contains("brand_new.txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_helpers_truncated() {
        // Test read functions with truncated data
        let empty: &[u8] = &[];
        let mut pos = 0;
        assert!(read_u8(empty, &mut pos).is_none());
        assert!(read_u32(empty, &mut pos).is_none());
        assert!(read_u64(empty, &mut pos).is_none());
        assert!(read_i64(empty, &mut pos).is_none());
    }

    #[test]
    fn test_deserialize_corrupt_data() {
        // Craft a minimal valid-looking undo db entry that's truncated
        let mut data = Vec::new();
        // Path length + path
        write_u32(&mut data, 4);
        data.extend_from_slice(b"test");
        // mtime secs + nanos
        write_i64(&mut data, 0);
        write_u32(&mut data, 0);
        // Undo group count
        write_u32(&mut data, 1);
        // Truncated here — group data missing
        let mut pos = 0;
        let result = deserialize_groups(&data[data.len() - 4..], &mut pos);
        // Should get Some with 1 group, but group deserialization will fail
        // Just verify it doesn't panic
        let _ = result;
    }

    #[test]
    fn test_entry_header_truncated() {
        let data = [0u8; 4]; // Just a path length, no actual path
        let result = entry_header(&data, 0);
        // path_len = 0, then needs at least i64 + u32 = 12 more bytes → should fail
        // Actually path_len from [0,0,0,0] is 0, so path is empty, then secs read fails
        assert!(result.is_none());
    }

    #[test]
    fn test_undo_history_no_db_file() {
        let dir = std::env::temp_dir().join("e_test_undo_nodb");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let db = dir.join("nonexistent.bin");
        fs::write(&path, b"hello").unwrap();

        let mut loaded = UndoStack::new();
        load_undo_history_from(&db, &path, &mut loaded);
        let (undo, redo) = loaded.stacks();
        assert!(undo.is_empty());
        assert!(redo.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }
}
