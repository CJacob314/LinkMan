use std::{
    env,
    ffi::{CStr, CString},
    fs, io, ptr,
};

use ansi_to_tui::IntoText;
use anyhow::{Context, Result, anyhow};
use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{self, Clear, ClearType},
};
use ratatui::{
    Frame, Terminal,
    layout::{Alignment, Constraint, Layout},
    prelude::Backend,
    style::Style,
    widgets::{Block, Borders, Paragraph},
};
use strip_ansi_escapes::strip_str;
use tui_input::{Input, backend::crossterm::EventHandler};

use crate::{ManPageInfo, text_handling};

/* TODO: Finish moving from the giant `run` function to this App struct, whose fields will have the
 * mutable app state and whose impl methods will do individual pieces of what the ungodly-big `run`
 * function does now. For example, we could have an `fn render` and the whole giant closure passed
 * to `terminal.draw()` could just become `terminal.draw(|frame| self.render(frame))?`.
 */
#[derive(Default, Debug)]
/// Struct to store app state
pub struct App {
    content: String,
    title: String,
    lines: Vec<String>,
    processed_content: String,
    num_lines: u16,
    scroll: u16,
    height: u16,
    mouse_mode: MouseMode,
    search_input: Input,
    search_mode: SearchMode,
}

impl App {
    pub(crate) fn new(content: String, man_page_id: impl AsRef<str>) -> Self {
        let title = format!("LinkMan - {}", man_page_id.as_ref());
        let lines: Vec<String> = strip_str(&content).lines().map(|s| s.to_owned()).collect();
        let processed_content = lines.join("\n");
        let num_lines = lines.len() as u16;

        Self {
            content,
            title,
            lines,
            processed_content,
            num_lines,
            ..Default::default()
        }
    }

    pub(crate) fn run<B>(mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B: ratatui::backend::Backend,
    {
        // SAFETY: This program has no "threads" in the sense that no two Linux tasks will ever share the same virtual memory space,
        // so this is safe.
        unsafe { set_man_width_variable()? };

        let mut stdout = io::stdout();

        execute!(
            stdout,
            Clear(ClearType::All),
            EnableMouseCapture, // Starting in MouseMode::LinkClicking
        )?;

        // Register panic handler to disable mouse capture
        let old_panic_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |hook_info| {
            drop(execute!(io::stdout(), DisableMouseCapture));
            old_panic_hook(hook_info);
        }));

        loop {
            terminal.draw(|frame| self.render(frame))?;

            if !self.handle_event(terminal)? {
                break;
            }
        }

        execute!(stdout, DisableMouseCapture)?;

        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        const SEARCH_PREFIX: &str = "Search: ";
        const SEARCH_PREFIX_LEN: u16 = SEARCH_PREFIX.len() as u16;

        let area = frame.area();
        self.height = area.height;
        self.scroll = self
            .scroll
            .min(self.num_lines.saturating_sub(self.height) + 2);

        // Split screen vertically into space for the content, and a single line for commands/searching
        let chunks = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).split(area);

        // Make content Paragraph
        let content_paragraph = Paragraph::new(
            self.processed_content
                .into_text()
                .expect("ansi_to_tui IntoText::into_text call failed"),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(self.title.as_str())
                .title_alignment(Alignment::Center),
        )
        .style(Style::default())
        .scroll((self.scroll, 0));

        frame.render_widget(content_paragraph, chunks[0]);

        // If the user's typing a search query...
        if self.search_mode == SearchMode::TypingQuery {
            let input_text = format!("{}{}", SEARCH_PREFIX, self.search_input.value());
            let input_paragraph = Paragraph::new(input_text);

            // Render typed query so far
            frame.render_widget(input_paragraph, chunks[1]);

            // Set cursor position
            let pos = self.search_input.visual_cursor() as u16;
            frame.set_cursor_position((pos + SEARCH_PREFIX_LEN, area.height));
        }
    }

    fn handle_event<B>(&mut self, terminal: &mut Terminal<B>) -> Result<bool>
    where
        B: Backend,
    {
        if self.search_mode == SearchMode::TypingQuery {
            match event::read()? {
                Event::Key(key) if key.code == KeyCode::Enter => self.perform_search()?,
                Event::Key(key) if key.code == KeyCode::Esc => self.cancel_search(),
                non_enter_event => drop(self.search_input.handle_event(&non_enter_event)),
            }

            return Ok(true);
        }

        match event::read()? {
            Event::Key(key) => match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _) => return Ok(false),
                (KeyCode::Down, _) | (KeyCode::Char('j'), _) => self.scroll += 1,
                (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                (KeyCode::Char('G'), _) | (KeyCode::Char('g'), KeyModifiers::SHIFT) => {
                    self.scroll = self.num_lines - self.height + 2
                }
                (KeyCode::Char('g'), _) => self.scroll = 0,
                (KeyCode::Char('i'), KeyModifiers::ALT) => self.toggle_mouse_mode()?,
                (KeyCode::Char('/'), _) => self.search_mode = SearchMode::TypingQuery,
                _ => (),
            },
            Event::Mouse(mouse_event)
                if matches!(mouse_event.kind, MouseEventKind::Up(MouseButton::Left))
                    && (1..=self.height - 3).contains(&mouse_event.row) =>
            {
                // SAFETY: Calling `word_at_position` from the same single thread every time is safe
                if let Some(word_clicked) = unsafe {
                    text_handling::word_at_position(
                        &self.lines,
                        self.scroll as usize,
                        mouse_event.row as usize,
                        mouse_event.column as usize,
                    )
                } {
                    // Ignoring failures (user probably just clicked on something that wasn't a link)
                    if let Ok(info) = <&str as TryInto<ManPageInfo>>::try_into(word_clicked) {
                        if try_link_jump(&info).is_ok() {
                            // There's no need to re-apply the program mouse mode unless man ran successfully (and therefore [probably] ran us again)

                            self.apply_mouse_mode()?;
                        }

                        // Clear terminal even if try_link_jump failed, since man will print a failure message we'll need to draw over if the man page doesn't exist
                        terminal.clear()?;
                    }
                }
            }
            Event::Mouse(mouse_event) if mouse_event.kind == MouseEventKind::ScrollDown => {
                self.scroll += 1;
            }
            Event::Mouse(mouse_event) if mouse_event.kind == MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            Event::Resize(cols, _) => {
                // Terminal resize event => recalculate needed variables
                // TODO: Evaluate how badly you need *THIS* textwrap::wrap call as well. I'm thinking you'll likely need this one a bit more than the last (already removed) one.
                self.lines = textwrap::wrap(strip_str(&self.content).as_str(), cols as usize)
                    .into_iter()
                    .map(|cow| cow.into_owned())
                    .collect();

                self.processed_content = self.lines.join("\n");
                self.num_lines = self.lines.len() as u16; // saturating cast is desired here

                // SAFETY: This program has no "threads" in the sense that no two Linux tasks will ever share the same virtual memory space,
                // so this is safe.
                unsafe { set_man_width_variable() }?;
            }
            _ => (),
        }

        Ok(true)
    }

    /// Toggles the [`App::mouse_mode`] (between [`MouseMode::LinkClicking`] and
    /// [`MouseMode::TextSelection`].
    fn toggle_mouse_mode(&mut self) -> Result<()> {
        let mut stdout = io::stdout();

        if matches!(self.mouse_mode, MouseMode::LinkClicking) {
            // Allow text selection by disabling mouse capture
            execute!(stdout, DisableMouseCapture)?;

            // Update program state
            self.mouse_mode = MouseMode::TextSelection;
        } else {
            // Allow link-clicking by enabling mouse capture
            execute!(stdout, EnableMouseCapture)?;

            // Update program state
            self.mouse_mode = MouseMode::LinkClicking;
        }

        Ok(())
    }

    /// Applies the current [`App::mouse_mode`] by enabling or disabling mouse capture.
    /// This is used to verify we are correctly handling user clicks after a man command successfully runs.
    fn apply_mouse_mode(&self) -> Result<()> {
        let mut stdout = io::stdout();

        match self.mouse_mode {
            MouseMode::LinkClicking => execute!(stdout, EnableMouseCapture)?,
            MouseMode::TextSelection => execute!(stdout, DisableMouseCapture)?,
        }

        Ok(())
    }

    fn cancel_search(&mut self) {
        self.search_input.reset();
        self.search_mode = SearchMode::NoSearch;
    }

    fn perform_search(&mut self) -> Result<()> {
        panic!(
            "TODO: Implement search. Text for which to search was: \"{}\"",
            self.search_input.value()
        );
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
pub(crate) unsafe fn set_man_width_variable() -> Result<()> {
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

pub(crate) fn exec_self(info: &ManPageInfo) -> Result<()> {
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

/// Holds two possible program states:
/// - `LinkClicking` may prevent text selection, but allows the user to click on, for example, `mount(2)` to open the `mount(2)` man-page.
///   In this mode, the program captures all mouse input.
/// - `TextSelection` will allow text selection, but does not allow the user to click on links.
///   They will either have to toggle the mode or use the keyboard to jump through a link (TODO).
#[derive(Copy, Clone, Debug, Default)]
enum MouseMode {
    #[default]
    LinkClicking,
    TextSelection,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum SearchMode {
    #[default]
    NoSearch,
    TypingQuery,
}

const MAN_PROGRAM: &CStr = c"man";
const SELF_PROGRAM: &str = "/proc/self/exe";
