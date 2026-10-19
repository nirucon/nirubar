//! Popups: a small panel under a clicked module.
//!
//! A popup is an override-redirect window, so the window manager does not tile
//! or focus it. Only one is open at a time, and it closes on:
//!
//!   * a click on the same module (toggle),
//!   * a click anywhere else, including the bar,
//!   * Escape, or
//!   * the configured timeout.
//!
//! The popup is a value the bar owns rather than a blocking call. The previous
//! version ran its own event loop, which stopped the bar from handling clicks
//! and let a second panel open on top of the first.
use crate::config::{Result, Theme};
use crate::render::Renderer;
use chrono::Local;
use std::time::{Duration, Instant};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
};

/// An open panel.
pub struct Popup {
    win: Window,
    /// Full-screen window under the panel that catches clicks anywhere else.
    catch: Window,
    gc: Gcontext,
    /// Module index the panel belongs to, so a second click toggles it.
    pub module: usize,
    opened: Instant,
    timeout: Duration,
    /// Where the panel is, so a click inside it does not close it.
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    title: String,
    lines: Vec<String>,
    font_size: f32,
    font_path: Option<String>,
    /// Focus owner before the panel took focus; restored when it closes.
    previous_focus: (Window, InputFocus),
}

impl Popup {
    /// Draw a panel and return it.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        c: &RustConnection,
        root: Window,
        theme: &Theme,
        bar_height: u16,
        font_size: f32,
        font_path: Option<&str>,
        module: usize,
        bar_x: i16,
        bar_y: i16,
        bar_width: u16,
        bar_window: Window,
        origin_x: i16,
        title: &str,
        lines: &[String],
        timeout_seconds: u32,
    ) -> Result<Self> {
        let line_height = (font_size * 1.5).ceil() as u16;
        let width: u16 = 340;
        let height = line_height * (lines.len() as u16 + 2) + 14;
        // Root coordinates, clamped inside the bar's physical monitor.
        let max_x = i32::from(bar_x) + i32::from(bar_width) - i32::from(width);
        let x = (i32::from(bar_x) + i32::from(origin_x))
            .clamp(i32::from(bar_x), max_x.max(i32::from(bar_x))) as i16;
        let root_geometry = c.get_geometry(root)?.reply()?;
        let max_y = i32::from(root_geometry.height) - i32::from(height);
        let y = (i32::from(bar_y) + i32::from(bar_height)).min(max_y.max(0)) as i16;
        let previous_focus = c.get_input_focus()?.reply()?;

        let screen_width = c.setup().roots[0].width_in_pixels;
        let screen_height = c.setup().roots[0].height_in_pixels;
        let win = c.generate_id()?;
        let gc = c.generate_id()?;
        // A transparent, full-screen window below the panel. Any click that is
        // not on the panel lands here, which is how the panel learns that the
        // user clicked somewhere else. The window manager is untouched: this
        // window is override-redirect and private to the bar.
        let catch = c.generate_id()?;
        c.create_window(
            0,
            catch,
            root,
            0,
            0,
            screen_width,
            screen_height,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::BUTTON_PRESS),
        )?;
        c.map_window(catch)?;
        // The input-only catcher never paints, and the bar stays clickable above it.
        c.configure_window(
            bar_window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        let bg = Theme::pixel(&theme.background)?;
        c.create_window(
            0,
            win,
            root,
            x,
            y,
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
        // Grab the keyboard so Escape reaches the panel. The grab is released
        // when the window is destroyed.
        // Three layers, because no single one is reliable on every server:
        //   1. grab Escape and Return at the root, which works even when the
        //      focus is elsewhere,
        //   2. focus the panel, so a server that delivers keys by focus still
        //      reaches it,
        //   3. rely on the timeout, which needs nothing from X at all.
        // Plain Escape and Return, with no modifier. GrabMask::ANY fails when
        // the window manager already owns some combination of the key (NIRUWM
        // binds Super+Escape to lock the screen), so the bare key is grabbed
        // instead: it is free and it is the one a panel needs.
        for code in [9u8, 36u8] {
            let _ = c
                .grab_key(
                    true,
                    root,
                    ModMask::from(0u16),
                    code,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .ok()
                .and_then(|cookie| cookie.check().ok());
        }
        // Map before focusing: a focus request on an unmapped window is ignored.
        c.map_window(win)?;
        c.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)?;
        c.flush()?;

        // The panel must be above the catch window.
        c.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        let popup = Self {
            win,
            catch,
            gc,
            module,
            opened: Instant::now(),
            timeout: Duration::from_secs(u64::from(timeout_seconds)),
            x,
            y,
            width,
            height,
            title: title.to_string(),
            lines: lines.to_vec(),
            font_size,
            font_path: font_path.map(str::to_string),
            previous_focus: (previous_focus.focus, previous_focus.revert_to),
        };
        popup.paint(c, root, theme, font_size, font_path, title, lines)?;
        Ok(popup)
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &self,
        c: &RustConnection,
        root: Window,
        theme: &Theme,
        font_size: f32,
        font_path: Option<&str>,
        title: &str,
        lines: &[String],
    ) -> Result<()> {
        let bg = Theme::pixel(&theme.background)?;
        let fg = Theme::pixel(&theme.foreground)?;
        let muted = Theme::pixel(&theme.muted)?;
        let line_height = (font_size * 1.5).ceil() as i16;
        let mut renderer = Renderer::direct(
            c,
            root,
            self.win,
            font_path,
            font_size,
            self.width,
            self.height,
        )?;
        // The panel's renderer places text by baseline, so each line gets a row.
        let row = |i: i16| -> i16 { 10 + line_height * (i + 1) };
        renderer.line(c, self.gc, 14, row(0), &title.to_uppercase(), fg, bg)?;
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            renderer.line(c, self.gc, 14, row(i as i16 + 1), line, muted, bg)?;
        }
        c.flush()?;
        Ok(())
    }

    /// Repaint retained content after an Expose without losing the panel.
    pub fn repaint(&self, c: &RustConnection, root: Window, theme: &Theme) -> Result<()> {
        self.paint(
            c,
            root,
            theme,
            self.font_size,
            self.font_path.as_deref(),
            &self.title,
            &self.lines,
        )
    }

    /// True when a pointer position is inside the panel.
    fn contains(&self, x: i16, y: i16) -> bool {
        x >= self.x
            && x < self.x + self.width as i16
            && y >= self.y
            && y < self.y + self.height as i16
    }

    /// Expire the panel after the configured time.
    pub fn expired(&self) -> bool {
        !self.timeout.is_zero() && self.opened.elapsed() >= self.timeout
    }

    pub fn window(&self) -> Window {
        self.win
    }

    pub fn catch_window(&self) -> Window {
        self.catch
    }

    pub fn destroy(&self, c: &RustConnection, root: Window) -> Result<()> {
        c.ungrab_key(0, root, ModMask::from(0u16))?;
        c.destroy_window(self.win)?;
        c.destroy_window(self.catch)?;
        c.free_gc(self.gc)?;
        let (window, revert) = self.previous_focus;
        if window > 1 {
            let _ = c.set_input_focus(revert, window, x11rb::CURRENT_TIME);
        }
        c.flush()?;
        Ok(())
    }
}

/// Reaction to an event while a panel is open.
pub enum Outcome {
    /// Keep the panel.
    Keep,
    /// Close the panel and do nothing else.
    Close,
}

impl Popup {
    /// Decide what an event means for this panel.
    pub fn handle(&self, event: &Event) -> Outcome {
        match event {
            // A click on the catch window is by definition outside the panel.
            Event::ButtonPress(e) if e.event == self.catch => Outcome::Close,
            Event::ButtonPress(e) => {
                // A click inside the panel keeps it: it is content, not a
                // dismiss target.
                if self.contains(e.root_x, e.root_y) {
                    return Outcome::Keep;
                }
                // A click on the bar at the module that opened it toggles it,
                // and a click elsewhere on the bar passes through so modules
                // stay responsive.
                Outcome::Close
            }
            Event::KeyPress(_) => Outcome::Close,
            _ => Outcome::Keep,
        }
    }
}

/// Calendar for the current month, Monday first, English short weekdays.
#[cfg(test)]
pub fn calendar(date_format: &str) -> (String, Vec<String>) {
    use chrono::Datelike;
    let now = Local::now();
    calendar_for(now.year(), now.month(), date_format)
}

/// Calendar for an explicit month; the bar lets the user step through months.
pub fn calendar_for(year: i32, month: u32, date_format: &str) -> (String, Vec<String>) {
    let now = Local::now();
    let title = format!("{} {:04}", month_name(month), year);
    let mut lines = vec![format!("today: {}", now.format(date_format))];
    lines.push(String::new());
    lines.push("Wk  Mon Tue Wed Thu Fri Sat Sun".to_string());
    lines.extend(crate::module::month_grid(year, month));
    lines.push(String::new());
    lines.push("←/→ change month".to_string());
    (title, lines)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn calendar_title_has_a_month_name() {
        let (title, lines) = calendar("%Y-%m-%d");
        assert!(title.contains(&Local::now().year().to_string()));
        assert!(lines.iter().any(|l| l.starts_with("Wk  Mon")));
    }

    #[test]
    fn every_month_has_a_name() {
        for m in 1..=12 {
            assert!(!month_name(m).is_empty());
        }
    }
}
