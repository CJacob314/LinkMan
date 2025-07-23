mod man_page_info;
mod program_mode;
mod text_handling;

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
    env,
    ffi::{CStr, CString},
    fs, io, ptr,
};
use strip_ansi_escapes::strip_str;

fn main() -> Result<()> {
    let mut stdout = io::stdout();

    let content = io::read_to_string(io::stdin())?;
    let man_string = text_handling::get_man_string(&content)?;

    /* First, check if we've received `--subsequent-run`. If we have, everything is dandy. If we
     * haven't, we'll need to parse the man page and section we were run on, set MANWIDTH, and
     * rerun the command. If we don't, the alignment will be wonky.
     */
    if env::args().skip(1).all(|s| &s != "--subsequent-run") {
        // SAFETY: This program has no "threads" in the sense that no two Linux tasks will ever share the same virtual memory space,
        // so this is safe.
        unsafe { set_man_width_variable() }?;
        let man_page_info = ManPageInfo::try_from(man_string.as_str())?;

        exec_self(&man_page_info)?;
    }

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

    let res = run(&mut terminal, &content, man_string);

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

fn run<B>(
    terminal: &mut Terminal<B>,
    content: impl AsRef<str>,
    man_page_id: impl AsRef<str>,
) -> Result<()>
where
    B: ratatui::backend::Backend,
{
    let title = format!("LinkMan - {}", man_page_id.as_ref());
    // TODO: Evaluate whether you need the Vec `lines` to hold owned Strings (you *might* be okay with holding just `&str`s)
    let mut lines: Vec<String> = strip_str(content.as_ref())
        .lines()
        .map(|s| s.to_owned())
        .collect();
    let mut processed_content = lines.join("\n");
    let mut num_lines = lines.len() as u16; // saturating cast is desired here
    let mut scroll: u16 = 0;
    let mut height = 0;

    // SAFETY: This program has no "threads" in the sense that no two Linux tasks will ever share the same virtual memory space,
    // so this is safe.
    unsafe { set_man_width_variable() }?;
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
                    .title(title.as_str())
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
                    text_handling::word_at_position(
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
                // TODO: Evaluate how badly you need *THIS* textwrap::wrap call as well. I'm thinking you'll likely need this one a bit more than the last (already removed) one.
                lines = textwrap::wrap(strip_str(content.as_ref()).as_str(), cols as usize)
                    .into_iter()
                    .map(|cow| cow.into_owned())
                    .collect();

                processed_content = lines.join("\n");
                num_lines = lines.len() as u16; // saturating cast is desired here

                // SAFETY: This program has no "threads" in the sense that no two Linux tasks will ever share the same virtual memory space,
                // so this is safe.
                unsafe { set_man_width_variable() }?;
            }
            _ => (),
        }
    }

    Ok(())
}

fn try_link_jump(info: &ManPageInfo) -> Result<()> {
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
                "Fork-child meant to run another man command terminated unsuccessfully"
            ))
        }
    } else {
        // Child
        exec_self(info).inspect_err(|e| {
            // This abnormal exit will be picked up by the parent's wait
            panic!("{e}");
        })
    }
}

fn exec_self(info: &ManPageInfo) -> Result<()> {
    let canonicalized_self_program = fs::canonicalize(SELF_PROGRAM)?;
    let pager = CString::new(format!(
        "{} --subsequent-run",
        canonicalized_self_program.display()
    ))?;

    let (man_section_number, man_name) = info.as_args()?;
    let args = [
        MAN_PROGRAM.as_ptr(),
        c"-P".as_ptr(),
        pager.as_ptr(),
        man_section_number.as_ptr(),
        man_name.as_ptr(),
        ptr::null(),
    ];

    if unsafe { libc::execvp(MAN_PROGRAM.as_ptr(), args.as_ptr()) } < 0 {
        Err(io::Error::last_os_error()).with_context(|| "libc::execvp call failed")
    } else {
        // SAFETY: libc::execvp will not return on success: only a -1 on failure
        unsafe {
            std::hint::unreachable_unchecked();
        }
    }
}

/// Sets the `MANWIDTH` environment variable to an appropriate width.
///
/// If `MANWIDTH` is already set and parsable as a [`u16`], this function simply returns
/// a [`std::result::Result::Ok`]. This is important since we are likely to be a child of another
/// `linkman` process that has already set `MANWIDTH` (and already subtracted 2).
/// Otherwise, it sets `MANWIDTH` to the number of terminal columns minus 2 (for the left and right
/// borders).
/// If the terminal size cannot be determined, it falls back to 78 (since `man(1)` also assumes a
/// default width of 80).
///
/// # NOTE
/// The caller of [`set_man_width_variable`] **must ensure** that there are no other threads
/// concurrently reading from or writing to any environment variables.
unsafe fn set_man_width_variable() -> Result<()> {
    // Return early if the `MANWIDTH` environment variable is set to a u16-parsable string
    if env::var("MANWIDTH")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .is_some()
    {
        return Ok(());
    }

    let manwidth = terminal::size()
        .map(|(cols, _)| cols)
        .unwrap_or(80)
        .saturating_sub(2);

    // SAFETY: Because the caller has upheld that no other threads are concurrently reading from or
    // writing to any other environment variables, this is safe. See `std::env::set_var`
    // documentation for more information.
    unsafe {
        env::set_var("MANWIDTH", format!("{manwidth}"));
    }

    Ok(())
}

const MAN_PROGRAM: &CStr = c"man";
const SELF_PROGRAM: &str = "/proc/self/exe";
