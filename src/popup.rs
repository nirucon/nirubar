//! Popups: a small panel under a clicked module.
//!
//! A popup is an override-redirect window, so the window manager does not tile
//! or focus it. It closes on the next click anywhere or on Escape, which is the
//! behaviour of every status bar people are used to.

use crate::config::{Result, Theme};
use crate::render::Renderer;
use chrono::{Datelike, Local};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
};

/// Calendar for the current month.
pub fn calendar(date_format: &str) -> (String, Vec<String>) {
    let now = Local::now();
    let title = format!("{} {:04}", month_name(now.month()), now.year());
    let mut lines = calendar_start(date_format);
    lines.push(String::new());
    lines.push("mo  tu  we  th  fr  sa  su".to_string());
    lines.extend(crate::module::month_grid(now.year(), now.month()));
    (title, lines)
}

/// Heading line: the configured date format, or the Swedish weekday order.
fn calendar_start(date_format: &str) -> Vec<String> {
    let now = Local::now();
    vec![format!("today: {}", now.format(date_format))]
}

fn month_name(m: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES.get(m as usize - 1).copied().unwrap_or("")
}

/// Draw and run one popup until it is dismissed.
///
/// `x` is the left edge in bar coordinates; `bar_height` is added so the panel
/// hangs directly below the bar.
#[allow(clippy::too_many_arguments)]
pub fn show(
    c: &RustConnection,
    root: Window,
    theme: &Theme,
    bar_height: u16,
    font_size: f32,
    font_path: Option<&str>,
    x: i16,
    title: &str,
    lines: &[String],
) -> Result<()> {
    let line_height = (font_size * 1.5).ceil() as u16;
    // Wide enough for the calendar row plus padding.
    let width: u16 = 340;
    let height = line_height * (lines.len() as u16 + 2) + 14;
    // Keep the panel on screen: a popup for the rightmost module would
    // otherwise be clipped at the edge and appear empty.
    let screen_width = c.setup().roots[0].width_in_pixels;
    let x = x
        .min(i16::try_from(screen_width.saturating_sub(width)).unwrap_or(0))
        .max(0);

    let win = c.generate_id()?;
    let gc = c.generate_id()?;
    let bg = Theme::pixel(&theme.background)?;
    c.create_window(
        0,
        win,
        root,
        x,
        bar_height as i16,
        width,
        height,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(bg)
            .border_pixel(Theme::pixel(&theme.border)?)
            .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS),
    )?;
    c.create_gc(gc, win, &CreateGCAux::new())?;
    // Grab the keyboard so Escape reaches this short-lived window.
    c.grab_keyboard(
        true,
        win,
        x11rb::CURRENT_TIME,
        GrabMode::ASYNC,
        GrabMode::ASYNC,
    )?;
    c.map_window(win)?;
    c.flush()?;

    let screen = &c.setup().roots[0];
    let fg = Theme::pixel(&theme.foreground)?;
    let muted = Theme::pixel(&theme.muted)?;
    let pad_x: i16 = 14;
    // The panel's `line` renderer places text by baseline, so every line gets
    // its own row: the title first, then the body from the next row down.
    let row = |i: u16| -> i16 { 10 + line_height as i16 * (i as i16 + 1) };

    let paint = |renderer: &mut Renderer| -> Result<()> {
        renderer.clear(c, gc, bg)?;
        renderer.line(c, gc, pad_x, row(0), &title.to_uppercase(), fg, bg)?;
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            renderer.line(c, gc, pad_x, row(i as u16 + 1), line, muted, bg)?;
        }
        Ok(())
    };

    let mut renderer = Renderer::direct(c, screen.root, win, font_path, font_size, width, height)?;
    paint(&mut renderer)?;
    c.flush()?;
    loop {
        match c.wait_for_event()? {
            Event::ButtonPress(_) => break,
            Event::KeyPress(e) => {
                // Escape or any key closes it: a popup is not a dialog.
                let _ = e;
                break;
            }
            Event::Expose(_) => {
                paint(&mut renderer)?;
                c.flush()?;
            }
            Event::DestroyNotify(w) if w.window == win => return Ok(()),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_title_has_a_month_name() {
        let (title, lines) = calendar("%Y-%m-%d");
        assert!(title.contains(&Local::now().year().to_string()));
        assert!(lines.iter().any(|l| l.starts_with("mo")));
    }

    #[test]
    fn month_names_are_complete() {
        for m in 1..=12 {
            assert!(!month_name(m).is_empty());
        }
        assert_eq!(month_name(10), "October");
    }
}
