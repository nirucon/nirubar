//! NIRUWM scratchpads: which slots hold a window and which one is shown.
//!
//! NIRUWM 0.17.4 publishes the root property `_NIRUWM_SCRATCHPADS`, one line
//! per slot: `slot<TAB>state<TAB>window<TAB>class`. Slot 0 is the default
//! scratchpad (Super+Shift+S), 1-9 the numbered ones (Super+Alt+N). `state` is
//! `shown`, `hidden`, or `empty` for a slot with a configured command that has
//! not been started (window 0). The property changes only when a scratchpad
//! does, and every root property change already wakes the bar, so the module
//! costs nothing while nothing happens. Titles are read from the windows only
//! when the panel opens.

use std::sync::{Mutex, OnceLock};

/// The latest state, written by the bar when the root property changes and
/// read by the module when it renders. One bar process serves one screen.
pub fn shared() -> &'static Mutex<Vec<Slot>> {
    static STATE: OnceLock<Mutex<Vec<Slot>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Vec::new()))
}

/// The current slots, cloned out of the shared state.
pub fn current() -> Vec<Slot> {
    shared().lock().map(|s| s.clone()).unwrap_or_default()
}

/// One slot as NIRUWM describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub slot: u8,
    pub shown: bool,
    /// 0 when the slot has a command but no window yet.
    pub window: u32,
    pub class: String,
}

/// Parse `_NIRUWM_SCRATCHPADS`. Unknown or malformed lines are skipped, so a
/// newer window manager can add fields without breaking an older bar.
pub fn parse(text: &str) -> Vec<Slot> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let slot: u8 = fields.next()?.trim().parse().ok()?;
            if slot > 9 {
                return None;
            }
            let state = fields.next()?.trim();
            let window: u32 = fields.next()?.trim().parse().ok()?;
            let class = fields.next().unwrap_or("").trim().to_string();
            match state {
                "shown" | "hidden" if window != 0 => Some(Slot {
                    slot,
                    shown: state == "shown",
                    window,
                    class,
                }),
                "empty" => Some(Slot {
                    slot,
                    shown: false,
                    window: 0,
                    class,
                }),
                _ => None,
            }
        })
        .collect()
}

/// The key that shows or hides a slot.
pub fn key(slot: u8) -> String {
    if slot == 0 {
        "Super+Shift+S".into()
    } else {
        format!("Super+Alt+{slot}")
    }
}

/// The bar text: nothing without a scratchpad window, the icon alone for one,
/// the icon and a count for several.
pub fn text(icon: &str, slots: &[Slot]) -> String {
    let windows = slots.iter().filter(|s| s.window != 0).count();
    match windows {
        0 => String::new(),
        1 => icon.to_string(),
        n => format!("{icon} {n}"),
    }
}

/// The application part of a `WM_CLASS` ("instance class" -> "class").
pub fn app(class: &str) -> &str {
    class.split_whitespace().last().unwrap_or(class)
}

/// One panel row: the key (which names the slot), title or application, and
/// whether it is shown. The panel is 340 px wide, so the row stays one line.
pub fn row(slot: &Slot, title: &str) -> String {
    let state = if slot.window == 0 {
        "start"
    } else if slot.shown {
        "shown"
    } else {
        "hidden"
    };
    let what = if slot.window != 0 && !title.trim().is_empty() {
        title.trim().to_string()
    } else {
        app(&slot.class).to_string()
    };
    let what: String = if what.chars().count() > 18 {
        what.chars().take(17).chain(std::iter::once('…')).collect()
    } else {
        what
    };
    let mark = if slot.shown { "●" } else { "○" };
    format!("{mark} {:<13} {what:<18} {state}", key(slot.slot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_window_manager_property() {
        let slots = parse("0\thidden\t11\tkitty scratchpad\n1\tempty\t0\t\n3\tshown\t12\tfirefox firefox\nbad line\n12\tshown\t5\tx\n");
        assert_eq!(slots.len(), 3);
        assert_eq!(
            slots[0],
            Slot {
                slot: 0,
                shown: false,
                window: 11,
                class: "kitty scratchpad".into()
            }
        );
        assert_eq!(slots[1].window, 0);
        assert!(slots[2].shown);
    }

    #[test]
    fn nothing_is_drawn_without_a_scratchpad_window() {
        let icon = "\u{f120}";
        assert_eq!(text(icon, &[]), "");
        assert_eq!(
            text(icon, &parse("2\tempty\t0\t\n")),
            "",
            "a configured slot alone is not a scratchpad"
        );
        assert_eq!(text(icon, &parse("0\thidden\t11\tk\n")), icon);
        assert_eq!(
            text(icon, &parse("0\thidden\t11\tk\n4\tshown\t9\tx\n")),
            format!("{icon} 2")
        );
    }

    #[test]
    fn panel_rows_name_the_key_the_slot_and_the_state() {
        let slots = parse("0\tshown\t11\tkitty scratchpad\n4\thidden\t9\tfirefox Firefox\n1\tempty\t0\tscratch-btop\n");
        assert_eq!(
            row(&slots[0], "~/src"),
            format!("● {:<13} {:<18} shown", "Super+Shift+S", "~/src")
        );
        assert_eq!(
            row(&slots[1], ""),
            format!("○ {:<13} {:<18} hidden", "Super+Alt+4", "Firefox")
        );
        assert!(row(&slots[2], "").ends_with("scratch-btop       start"));
        let long = row(&slots[1], "a very long window title that does not fit");
        assert!(long.contains('…'));
        assert!(long.chars().count() <= 42, "{long}");
    }
}
