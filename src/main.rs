mod buffer;
mod clipboard;
mod command;
mod command_buffer;
mod document;
mod editor;
mod file_io;
#[allow(unused)]
mod highlight;
mod keybind;
mod language;
mod operation;
mod render;
mod selection;
#[allow(unused)]
mod signal;
mod view;

use std::io::{self, Write};
use std::path::Path;
use std::process;

fn confirm(prompt: &str) -> bool {
    eprint!("{} (y/n) ", prompt);
    io::stderr().flush().unwrap();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).unwrap();
    buf.trim().eq_ignore_ascii_case("y")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 2 {
        eprintln!("Usage: e [file]");
        process::exit(1);
    }

    let (text, filename) = if args.len() > 1 {
        let path = Path::new(&args[1]);
        if path.exists() {
            // File safety checks
            match file_io::file_size(path) {
                Ok(size) if size > 5_000_000 => {
                    if !confirm(&format!(
                        "e: {} is {}MB. Open anyway?",
                        args[1],
                        size / 1_000_000
                    )) {
                        process::exit(0);
                    }
                }
                _ => {}
            }

            match file_io::read_file(path) {
                Ok(data) => {
                    if file_io::is_likely_binary(&data)
                        && !confirm(&format!(
                            "e: {} appears to be binary. Open anyway?",
                            args[1]
                        ))
                    {
                        process::exit(0);
                    }
                    (data, Some(args[1].clone()))
                }
                Err(e) => {
                    eprintln!("e: {}: {}", args[1], e);
                    process::exit(1);
                }
            }
        } else {
            // New file — empty buffer with name set
            (Vec::new(), Some(args[1].clone()))
        }
    } else {
        (Vec::new(), None)
    };

    let mut ed = editor::Editor::new(text, filename);
    if let Err(e) = ed.run() {
        eprintln!("e: {}", e);
        process::exit(1);
    }
}
