use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;
use std::process;

use e::{editor, file_io};

fn confirm(prompt: &str) -> bool {
    // Use alternate screen so the prompt doesn't pollute terminal history
    eprint!("\x1b[?1049h\x1b[2J\x1b[H{} (y/n) ", prompt);
    let _ = io::stderr().flush();
    let mut buf = String::new();
    if io::stdin().read_line(&mut buf).is_err() {
        eprint!("\x1b[?1049l");
        let _ = io::stderr().flush();
        return false;
    }
    let result = buf.trim().eq_ignore_ascii_case("y");
    eprint!("\x1b[?1049l");
    let _ = io::stderr().flush();
    result
}

type LoadResult = Result<Option<(Vec<u8>, Option<String>)>, String>;

/// Load file data from command-line args. Returns Ok(Some((text, filename))) on success,
/// Ok(None) if user declined a confirmation prompt, or Err on failure.
fn load(
    args: &[String],
    piped_stdin: bool,
    stdin_data: Option<Vec<u8>>,
    confirm_fn: impl Fn(&str) -> bool,
) -> LoadResult {
    if args.len() > 2 {
        return Err("Usage: e [file]".to_string());
    }

    if args.len() > 1 {
        let path = Path::new(&args[1]);
        if path.exists() {
            // File safety checks
            if let Ok(size) = file_io::file_size(path)
                && size > 5_000_000
                && !confirm_fn(&format!(
                    "e: {} is {}MB. Open anyway?",
                    args[1],
                    size / 1_000_000
                ))
            {
                return Ok(None);
            }

            let data = file_io::read_file(path).map_err(|e| format!("{}: {}", args[1], e))?;

            if file_io::is_likely_binary(&data)
                && !confirm_fn(&format!(
                    "e: {} appears to be binary. Open anyway?",
                    args[1]
                ))
            {
                return Ok(None);
            }

            Ok(Some((data, Some(args[1].clone()))))
        } else {
            // New file — empty buffer with name set
            Ok(Some((Vec::new(), Some(args[1].clone()))))
        }
    } else if piped_stdin {
        Ok(Some((stdin_data.unwrap_or_default(), None)))
    } else {
        Ok(Some((Vec::new(), None)))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() == 2 && (args[1] == "-V" || args[1] == "--version") {
        println!("e {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let piped_stdin = !io::stdin().is_terminal();

    let stdin_data = if piped_stdin {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf).unwrap_or_else(|e| {
            eprintln!("e: failed to read stdin: {}", e);
            process::exit(1);
        });
        Some(buf)
    } else {
        None
    };

    let (text, filename) = match load(&args, piped_stdin, stdin_data, confirm) {
        Ok(Some(t)) => t,
        Ok(None) => process::exit(0),
        Err(e) => {
            eprintln!("e: {}", e);
            process::exit(1);
        }
    };

    // Acquire file lock
    if let Some(ref name) = filename {
        let path = Path::new(name);
        if file_io::acquire_lock(path).is_err() {
            if !confirm(&format!(
                "e: {} is already open in another instance. Delete lock and open anyway?",
                name
            )) {
                process::exit(0);
            }
            file_io::release_lock(path);
            if let Err(e) = file_io::acquire_lock(path) {
                eprintln!("e: {}", e);
                process::exit(1);
            }
        }
    }

    let mut ed = editor::Editor::new(text, filename.clone(), piped_stdin);
    let result = ed.run();

    // Release file lock
    if let Some(ref name) = filename {
        file_io::release_lock(Path::new(name));
    }

    if let Err(e) = result {
        eprintln!("e: {}", e);
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_load_too_many_args() {
        let result = load(&args(&["e", "a", "b"]), false, None, |_| true);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Usage"));
    }

    #[test]
    fn test_load_no_args_no_pipe() {
        let result = load(&args(&["e"]), false, None, |_| true);
        let (text, filename) = result.unwrap().unwrap();
        assert!(text.is_empty());
        assert!(filename.is_none());
    }

    #[test]
    fn test_load_piped_stdin() {
        let data = b"hello from pipe".to_vec();
        let result = load(&args(&["e"]), true, Some(data.clone()), |_| true);
        let (text, filename) = result.unwrap().unwrap();
        assert_eq!(text, data);
        assert!(filename.is_none());
    }

    #[test]
    fn test_load_new_file() {
        let result = load(
            &args(&["e", "/tmp/e_test_nonexistent_file_xyz"]),
            false,
            None,
            |_| true,
        );
        let (text, filename) = result.unwrap().unwrap();
        assert!(text.is_empty());
        assert_eq!(
            filename.as_deref(),
            Some("/tmp/e_test_nonexistent_file_xyz")
        );
    }

    #[test]
    fn test_load_existing_file() {
        let dir = std::env::temp_dir().join("e_test_main_load");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("hello.txt");
        std::fs::write(&path, b"hello world").unwrap();

        let result = load(&args(&["e", path.to_str().unwrap()]), false, None, |_| true);
        let (text, filename) = result.unwrap().unwrap();
        assert_eq!(text, b"hello world");
        assert_eq!(filename.as_deref(), Some(path.to_str().unwrap()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_binary_declined() {
        let dir = std::env::temp_dir().join("e_test_main_binary");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("data.bin");
        // Write enough null bytes to trigger binary detection
        let mut data = vec![0u8; 512];
        data[0] = 0x7f; // ELF-like header
        std::fs::write(&path, &data).unwrap();

        let result = load(
            &args(&["e", path.to_str().unwrap()]),
            false,
            None,
            |_| false, // decline
        );
        // User declined — should return Ok(None)
        assert!(result.unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_binary_accepted() {
        let dir = std::env::temp_dir().join("e_test_main_binary_ok");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("data.bin");
        let mut data = vec![0u8; 512];
        data[0] = 0x7f;
        std::fs::write(&path, &data).unwrap();

        let result = load(
            &args(&["e", path.to_str().unwrap()]),
            false,
            None,
            |_| true, // accept
        );
        let (text, filename) = result.unwrap().unwrap();
        assert_eq!(text, data);
        assert!(filename.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
