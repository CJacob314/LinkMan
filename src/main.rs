mod app;
mod man_page_info;
mod text_handling;

use anyhow::Result;
use app::App;
use man_page_info::ManPageInfo;
use std::{env, io};

fn main() -> Result<()> {
    let content = io::read_to_string(io::stdin())?;
    let man_string = text_handling::get_man_string(&content)?;

    /* First, check if we've received `--subsequent-run`. If we have, everything is dandy. If we
     * haven't, we'll need to parse the man page and section we were run on, set MANWIDTH, and
     * rerun the command. If we don't, the alignment will be wonky.
     */
    if env::args().skip(1).all(|s| &s != "--subsequent-run") {
        // SAFETY: This program has no "threads" in the sense that no two Linux tasks will ever share the same virtual memory space,
        // so this is safe.
        unsafe { app::set_man_width_variable() }?;
        let man_page_info = ManPageInfo::try_from(man_string.as_str())?;

        app::exec_self(&man_page_info)?;
    }

    // Replace stdin fd with PTY/TTY fd from stderr
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDIN_FILENO) } < 0 {
        // SAFETY: Simple dup2 call made with two valid fds. There is valid error checking: program will panic if dup2 fails
        panic!("libc::dup2 call (to put /dev/tty fd over stdin fd) failed");
    }

    // Setup terminal
    let mut terminal = ratatui::init();

    let app = App::new(content, man_string);
    let res = app.run(&mut terminal);

    // Restore terminal
    ratatui::restore();
    terminal.show_cursor()?;

    // Now that we've restored the terminal, return a Result::Err returned by `run` up.
    res?;

    // Successful exit
    Ok(())
}
