mod man_page_info;
mod program_mode;

use ansi_to_tui::IntoText;
use anyhow::{Context, Result, anyhow};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use man_page_info::ManPageInfo;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Alignment,
    style::Style,
    widgets::{Block, Borders, Paragraph},
};
use std::{
    ffi::{CStr, CString},
    fs, io,
    os::unix::ffi::OsStrExt,
    process, ptr,
};
use strip_ansi_escapes::strip_str;

fn main() -> Result<()> {
    let mut stdout = io::stdout();

    let content = io::read_to_string(io::stdin())?;

    // Replace stdin fd with PTY/TTY fd from stderr
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDIN_FILENO) } < 0 {
        // SAFETY: Simple dup2 call made with two valid fds. There is valid error checking: program will panic if dup2 fails
        panic!("libc::dup2 call (to put /dev/tty fd over stdin fd) failed");
    }

    // Setup terminal
    terminal::enable_raw_mode()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        Clear(ClearType::All),
        EnableMouseCapture, // Starting in MouseMode::LinkClicking
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal, &content);

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
    )?;
    terminal.show_cursor()?;

    // Now that we've restored the terminal, return a Result::Err returned by `run` up.
    res?;

    // Successful exit
    Ok(())
}

fn run<B>(terminal: &mut Terminal<B>, content: impl AsRef<str>) -> Result<()>
where
    B: ratatui::backend::Backend,
{
    let mut lines: Vec<String> = textwrap::wrap(
        strip_str(content.as_ref()).as_str(),
        terminal::size()?.0 as usize,
    )
    .into_iter()
    .map(|cow| cow.into_owned())
    .collect();
    let mut processed_content = lines.join("\n");
    let mut num_lines = lines.len() as u16; // saturating cast is desired here
    let mut scroll: u16 = 0;
    let mut height = 0;
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            height = area.height;
            scroll = scroll.min(num_lines.saturating_sub(height) + 2);

            let paragraph = Paragraph::new(
                processed_content
                    .into_text()
                    .expect("ansi_to_tui IntoText::into_text call failed"),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("LinkMan")
                    .title_alignment(Alignment::Center),
            )
            .style(Style::default())
            .scroll((scroll, 0));

            frame.render_widget(paragraph, area);
        })?;

        match event::read()? {
            Event::Key(key) => match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _) => break,
                (KeyCode::Down, _) | (KeyCode::Char('j'), _) => scroll += 1,
                (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                    scroll = scroll.saturating_sub(1);
                }
                (KeyCode::Char('G'), _) | (KeyCode::Char('g'), KeyModifiers::SHIFT) => {
                    scroll = num_lines - height + 2
                }
                (KeyCode::Char('g'), _) => scroll = 0,
                (KeyCode::Char('i'), KeyModifiers::ALT) => unsafe {
                    // SAFETY: This program is single-threaded
                    program_mode::toggle()?
                },
                _ => (),
            },
            Event::Mouse(mouse_event)
                if matches!(mouse_event.kind, MouseEventKind::Up(MouseButton::Left)) =>
            {
                // SAFETY: Calling `word_at_position` from the same single thread every time is safe
                if let Some(word_clicked) = unsafe {
                    word_at_position(
                        &lines,
                        scroll as usize,
                        mouse_event.row as usize,
                        mouse_event.column as usize,
                    )
                } {
                    // Ignoring failures (user probably just clicked on something that wasn't a link)
                    if let Ok(info) = <&str as TryInto<ManPageInfo>>::try_into(word_clicked) {
                        if try_link_jump(&info).is_ok() {
                            // There's no need to re-apply the program mouse mode unless man ran successfully (and therefore [probably] ran us again)

                            // SAFETY:  Calling `apply_program_mode` from the same single thread every time is safe
                            unsafe { program_mode::apply()? };
                        }

                        // Clear terminal even if try_link_jump failed, since man will print a failure message we'll need to draw over if the man page doesn't exist
                        terminal.clear()?;
                    }
                }
            }
            Event::Mouse(mouse_event) if mouse_event.kind == MouseEventKind::ScrollDown => {
                scroll += 1;
            }
            Event::Mouse(mouse_event) if mouse_event.kind == MouseEventKind::ScrollUp => {
                scroll = scroll.saturating_sub(1);
            }
            Event::Resize(cols, _) => {
                // Terminal resize event => recalculate needed variables
                lines = textwrap::wrap(strip_str(content.as_ref()).as_str(), cols as usize)
                    .into_iter()
                    .map(|cow| cow.into_owned())
                    .collect();

                processed_content = lines.join("\n");
                num_lines = lines.len() as u16; // saturating cast is desired here
            }
            _ => (),
        }
    }

    Ok(())
}

/// Returns a reference ([`&str`]) the word at the given position in the given lines of text.
///
/// # NOTE
/// This function is `unsafe` because it can only be called from a single-threaded context.
/// This is due to the fact that it uses a `static mut` map to cache computations.
unsafe fn word_at_position(
    lines: &[String],
    scroll: usize,
    row: usize,
    mut col: usize,
) -> Option<&str> {
    use unicode_segmentation::UnicodeSegmentation;

    col = col.checked_sub(1)?;

    // Module in place to prevent accidental direct use of `static mut` pointer `LINE_OFFSETS_CACHE`.
    mod offsets_cache {
        use std::collections::HashMap;
        use std::ptr;
        static mut LINE_OFFSETS_CACHE: *mut HashMap<String, Vec<usize>> = ptr::null_mut();

        pub(super) fn get_cache<'a>() -> &'a mut HashMap<String, Vec<usize>> {
            unsafe {
                if LINE_OFFSETS_CACHE.is_null() {
                    LINE_OFFSETS_CACHE = Box::into_raw(Box::new(HashMap::new()));
                }
                &mut *LINE_OFFSETS_CACHE
            }
        }
    }

    let line = lines.get(row.checked_add(scroll)?.checked_sub(1)?)?;

    // Group line by Unicode extended grapheme clusters, as recommended by [UAX #29](https://www.unicode.org/reports/tr29/#Grapheme_Cluster_Boundaries)
    let graphemes: Vec<&str> = UnicodeSegmentation::graphemes(line.as_str(), true).collect();

    if col >= graphemes.len() {
        // Column is out of bounds
        return None;
    }

    // If the grapheme is whitespace, return None
    if graphemes[col].chars().all(char::is_whitespace) {
        return None;
    }

    // Walk backward to find the start of the word
    let mut start = col;
    while start > 0
        && !graphemes[start - 1]
            .chars()
            .all(|c| char::is_whitespace(c) || c == '/')
    {
        start -= 1;
    }

    // Walk forward to find the end of the word
    let mut end = col;
    while end < graphemes.len()
        && !graphemes[end]
            .chars()
            .all(|c| char::is_whitespace(c) || c == '/')
    {
        end += 1;
    }

    // TODO: Benchmark this code with vs. without the cache and use whichever version was faster
    let offsets_cache = offsets_cache::get_cache();
    if let Some(offsets) = offsets_cache.get(line.as_str()) {
        // Cached offsets were present. Use those to compute returned string slice.
        let start_byte = offsets[start];
        let end_byte = offsets[end];

        Some(&line[start_byte..end_byte])
    } else {
        // Compute cached offsets
        let mut byte_offsets = Vec::with_capacity(graphemes.len() + 1);
        let mut offset_accum = 0;
        byte_offsets.push(offset_accum);
        for grapheme in &graphemes {
            offset_accum += grapheme.len();
            byte_offsets.push(offset_accum);
        }

        let start_byte = byte_offsets[start];
        let end_byte = byte_offsets[end];

        // Update cache
        offsets_cache.insert(line.clone(), byte_offsets);

        Some(&line[start_byte..end_byte])
    }
}

fn try_link_jump(info: &ManPageInfo) -> Result<()> {
    const MAN_PROGRAM: &CStr = c"man";
    const SELF_PROGRAM: &str = "/proc/self/exe";

    let canonicalized_self_program =
        CString::new(fs::canonicalize(SELF_PROGRAM)?.into_os_string().as_bytes())?;

    let (man_section_number, man_name) = info.as_args()?;
    let args = [
        MAN_PROGRAM.as_ptr(),
        c"-P".as_ptr(),
        canonicalized_self_program.as_ptr(),
        man_section_number.as_ptr(),
        man_name.as_ptr(),
        ptr::null(),
    ];

    // SAFETY:: Write this (TODO)
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error()).with_context(|| "libc::fork failed");
    }

    if pid > 0 {
        // Parent
        let mut status = 0_i32;
        if unsafe { libc::wait(&raw mut status) } < 0 {
            return Err(io::Error::last_os_error()).with_context(|| "libc::wait in parent failed");
        }

        if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == libc::EXIT_SUCCESS {
            Ok(())
        } else {
            Err(anyhow!(
                "Child meant to run another man command terminated unsuccessfully"
            ))
        }
    } else {
        // Child
        if unsafe { libc::execvp(MAN_PROGRAM.as_ptr(), args.as_ptr()) } < 0 {
            // This abnormal exit will be picked up by the parent's wait
            process::abort();
        } else {
            // SAFETY: libc::execvp will not return on success: only a -1 on failure
            unsafe {
                std::hint::unreachable_unchecked();
            }
        }
    }
}
