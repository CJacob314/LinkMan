use ansi_to_tui::IntoText;
use anyhow::Result;
use crossterm::{
	event::{
		self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseButton, MouseEventKind,
	},
	execute,
	terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
	Terminal,
	backend::CrosstermBackend,
	layout::{Alignment, Constraint, Layout},
	style::Style,
	widgets::{Block, Borders, Clear, Paragraph},
};
use std::io;
use strip_ansi_escapes::strip_str;

fn main() -> Result<()> {
	let mut stdout = io::stdout();

	let content = io::read_to_string(io::stdin())?;

	// Replace stdin fd with PTY/TTY fd from stderr
	// SAFETY: Simple dup2 call made with two valid fds. There is valid error checking: program will panic if dup2 fails
	if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDIN_FILENO) } < 0 {
		panic!("libc::dup2 call (to put /dev/tty fd over stdin fd) failed");
	}

	// Setup terminal
	terminal::enable_raw_mode()?;
	execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;

	let res = run(&mut terminal, &content);

	// Restore terminal
	terminal::disable_raw_mode()?;
	execute!(
		terminal.backend_mut(),
		LeaveAlternateScreen,
		DisableMouseCapture
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
	let mut clicked_text = Paragraph::default();
	let lines: Vec<String> = strip_str(content.as_ref())
		.lines()
		.map(|s| s.to_owned())
		.collect();
	let content = content.as_ref().as_bytes();
	let num_lines = content.iter().filter(|&b| *b == b'\n').count() as u16; // saturating cast is desired here
	let mut scroll: u16 = 0;
	let mut height = 0;
	loop {
		terminal.draw(|frame| {
			let area = frame.area();
			height = area.height;
			scroll = scroll.min(num_lines - height + 2);

			// TODO: Allow text wrapping here (and adjust `word_at_position` function as needed)
			let paragraph = Paragraph::new(
				content
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
			let vertical_chunks =
				Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).split(area);
			let layouts = Layout::horizontal([Constraint::Length(20), Constraint::Min(0)])
				.split(vertical_chunks[1]);
			frame.render_widget(Clear, layouts[0]);
			frame.render_widget(&clicked_text, layouts[0]);
		})?;

		match event::read()? {
			Event::Key(key) => match key.code {
				KeyCode::Char('q') => break,
				KeyCode::Down | KeyCode::Char('j') => scroll += 1,
				KeyCode::Up | KeyCode::Char('k') => {
					scroll = scroll.saturating_sub(1);
				}
				KeyCode::Char('g') => scroll = 0,
				KeyCode::Char('G') => scroll = num_lines - height + 2,
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
					let mut info = String::from("Word: ");
					info.push_str(word_clicked);
					info.push_str(
						format!(
							"\n\nPosition: ({}, {})",
							mouse_event.row, mouse_event.column
						)
						.as_str(),
					);
					clicked_text = Paragraph::new(info).block(
						Block::default()
							.borders(Borders::ALL)
							.title("ClickedWordInfo")
							.title_alignment(Alignment::Center),
					);
					// TODO: Actually implement the "link" of "LinkMan" here
				}
			}
			Event::Mouse(mouse_event) if mouse_event.kind == MouseEventKind::ScrollDown => {
				scroll += 1;
			}
			Event::Mouse(mouse_event) if mouse_event.kind == MouseEventKind::ScrollUp => {
				scroll = scroll.saturating_sub(1);
			}
			_ => (),
		}
	}

	Ok(())
}

/// Returns a reference ([`&str`]) the word at the given position in the given lines of text.
///
/// # Note
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
	while start > 0 && !graphemes[start - 1].chars().all(char::is_whitespace) {
		start -= 1;
	}

	// Walk forward to find the end of the word
	let mut end = col;
	while end < graphemes.len() && !graphemes[end].chars().all(char::is_whitespace) {
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
