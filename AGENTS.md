This is a minimalistic and performant text editor.

Please update this file before every commit.

## design constraints

- rust 2024 edition
- no tabs, no file browsers. just purely edit a file
- only support macos and linux

## inspiration

https://github.com/ilai-deutel/kibi
- a good resource. we don't need to be this lines-of-code constrained, and i'd want to use an actual low level tui library

https://github.com/micro-editor/micro
- a lot of the features and feature design i want are from this editor

## v0 feature checklist
[] performant repainting (be smart about only repainting what's needed)
[] handle window resize
[] mouse support (click, drag to select, double click to select by word, triple click to select by line)
[] hold shift and use arrow keys to select text
[] copypaste from system clipboard (linux and macos with platform detection)
[] emacs style keybindings and basic commands
  [] save (^s)
  [] kill line (^k)
  [] goto top of file (^t), end of file (^g)
  [] undo (^z), redo (^y)
[] regex find (^f) then Tab to tab through results
[] open editor command (cmd+p) buffer where commands can be typed and executed
  - all editor functions like saving files, find and replace, are really just all commands
[] goto line number (^l) -> opens a command buffer to enter the linenumber
[] command: `replaceall findregex string` that applies to the selection if there is one, otherwise the entire file

more advanced features (save for later, just keep in mind when designing the foundation)
[] performant and basic (not too noisy with colors) syntax highlighting
[] command: `comment on|off` comments out / uncomments the selected code
[] when saving a file, if parent directories are not created then prompt to confirm a `mkdir -p`
[] if permissions denied when saving file
[] automatic formatting with file detection
  - indent style space, 2 spaces for all files except Makefile, .c, .go where the indent needs to be tab
  - tab characters need to display as 2 spaces
