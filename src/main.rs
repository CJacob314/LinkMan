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
	layout::Alignment,
	style::Style,
	widgets::{Block, Borders, Paragraph, Wrap},
};
use std::io::{self, Read};

fn main() -> Result<()> {
	let mut stdout = io::stdout();

	let mut content = Vec::new();
	io::stdin().read_to_end(&mut content)?;

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

fn run<B>(terminal: &mut Terminal<B>, content: &[u8]) -> Result<()>
where
	B: ratatui::backend::Backend,
{
	let num_lines = content.iter().filter(|&b| *b == b'\n').count() as u16; // saturating cast is desired here
	let mut scroll: u16 = 0;
	let mut height = 0;
	loop {
		terminal.draw(|frame| {
			let area = frame.area();
			height = area.height;
			scroll = scroll.min(num_lines - height + 2);
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
			.scroll((scroll, 0))
			.wrap(Wrap { trim: false });

			frame.render_widget(paragraph, area);
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
				// TODO: Handle clicks
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
