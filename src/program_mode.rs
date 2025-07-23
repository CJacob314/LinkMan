use std::io;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
};

/// Holds two possible program states:
/// - `LinkClicking` may prevent text selection, but allows the user to click on, for example, `mount(2)` to open the `mount(2)` man-page.
///   In this mode, the program captures all mouse input.
/// - `TextSelection` will allow text selection, but does not allow the user to click on links.
///   They will either have to toggle the mode or use the keyboard to jump through a link (TODO).
#[derive(Copy, Clone)]
enum MouseMode {
    LinkClicking,
    TextSelection,
}

static mut PROGRAM_MODE: MouseMode = MouseMode::LinkClicking;

/// Toggles the [`PROGRAM_MODE`] (between [`MouseMode::LinkClicking`] and
/// [`MouseMode::TextSelection`].
///
/// # NOTE
/// This function **must only be called from a single thread** due to writing to a `static mut` variable.
pub(crate) unsafe fn toggle() -> Result<()> {
    let mut stdout = io::stdout();

    if unsafe { matches!(PROGRAM_MODE, MouseMode::LinkClicking) } {
        // Allow text selection by disabling mouse capture
        execute!(stdout, DisableMouseCapture)?;

        // Update program state variable
        unsafe { PROGRAM_MODE = MouseMode::TextSelection };
    } else {
        // Allow link-clicking by enabling mouse capture
        execute!(stdout, EnableMouseCapture)?;

        // Update program state variable
        unsafe { PROGRAM_MODE = MouseMode::LinkClicking };
    }

    Ok(())
}

/// Applies the current [`PROGRAM_MODE`] by enabling or disabling mouse capture.
/// This is used to verify we are correctly handling user clicks after a man command successfully runs.
///
/// # NOTE
/// This function **must only be called from a single thread** due to reading from a `static mut` variable.
pub(crate) unsafe fn apply() -> Result<()> {
    let mut stdout = io::stdout();

    match unsafe { PROGRAM_MODE } {
        MouseMode::LinkClicking => execute!(stdout, EnableMouseCapture)?,
        MouseMode::TextSelection => execute!(stdout, DisableMouseCapture)?,
    }

    Ok(())
}
