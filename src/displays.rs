//! Connected outputs from RandR, kept current by RandR events.
//!
//! The display module used to run `xrandr --current` every five seconds in
//! every bar. Now one thread per bar holds its own X connection, asks RandR
//! once, and then sleeps until the server reports a screen, output or CRTC
//! change. Without RandR the old `xrandr` query is used.

use std::sync::{Arc, Mutex, OnceLock};

use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _, NotifyMask};
use x11rb::protocol::xproto::Window;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use crate::status::DisplayOutput;

type State = Arc<Mutex<Option<Vec<DisplayOutput>>>>;

static STATE: OnceLock<Option<State>> = OnceLock::new();

/// The outputs RandR reports now, in the server's order, like the connected
/// lines of `xrandr --current`.
pub fn query(c: &RustConnection, root: Window) -> Option<Vec<DisplayOutput>> {
    // RandR 1.3 for the current-state request; servers answer per client.
    c.randr_query_version(1, 3).ok()?.reply().ok()?;
    let resources = c
        .randr_get_screen_resources_current(root)
        .ok()?
        .reply()
        .ok()?;
    let primary = c
        .randr_get_output_primary(root)
        .ok()?
        .reply()
        .map(|reply| reply.output)
        .unwrap_or(0);
    let infos: Vec<_> = resources
        .outputs
        .iter()
        .filter_map(|&output| {
            c.randr_get_output_info(output, resources.config_timestamp)
                .ok()
                .map(|cookie| (output, cookie))
        })
        .collect();
    let mut outputs = Vec::new();
    for (output, cookie) in infos {
        let Ok(info) = cookie.reply() else { continue };
        if info.connection != randr::Connection::CONNECTED {
            continue;
        }
        let crtc = (info.crtc != 0)
            .then(|| {
                c.randr_get_crtc_info(info.crtc, resources.config_timestamp)
                    .ok()?
                    .reply()
                    .ok()
            })
            .flatten()
            .filter(|crtc| crtc.mode != 0 && crtc.width > 0 && crtc.height > 0);
        outputs.push(DisplayOutput {
            name: String::from_utf8_lossy(&info.name).into_owned(),
            resolution: crtc
                .as_ref()
                .map(|crtc| format!("{}x{}", crtc.width, crtc.height))
                .unwrap_or_else(|| "?".into()),
            primary: output == primary,
            position: crtc
                .as_ref()
                .map(|crtc| (i32::from(crtc.x), i32::from(crtc.y)))
                .unwrap_or((0, 0)),
        });
    }
    Some(outputs)
}

fn connect() -> Option<(RustConnection, Window)> {
    let (c, screen) = x11rb::connect(None).ok()?;
    let root = c.setup().roots.get(screen)?.root;
    let version = c.randr_query_version(1, 3).ok()?.reply().ok()?;
    (version.major_version > 1 || version.minor_version >= 3).then_some((c, root))
}

fn watch(c: RustConnection, root: Window, state: State) {
    let mask = NotifyMask::SCREEN_CHANGE | NotifyMask::OUTPUT_CHANGE | NotifyMask::CRTC_CHANGE;
    if c.randr_select_input(root, mask).is_err() || c.flush().is_err() {
        return;
    }
    // A query may have raced with a change before the events were selected.
    let mut current = query(&c, root);
    loop {
        if let Ok(mut slot) = state.lock() {
            if *slot != current {
                *slot = current.clone();
                drop(slot);
                crate::wake_update();
            }
        }
        let Ok(event) = c.wait_for_event() else {
            return;
        };
        if !matches!(
            event,
            Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_)
        ) {
            continue;
        }
        // One hotplug is a burst of notifications: answer them together.
        while let Ok(Some(_)) = c.poll_for_event() {}
        current = query(&c, root);
    }
}

/// True when RandR events keep the outputs current; otherwise each call of
/// `outputs` runs `xrandr`, and callers should throttle.
pub fn live() -> bool {
    STATE.get().is_some_and(Option::is_some)
}

/// The current outputs, `None` when neither RandR nor `xrandr` answers.
pub fn outputs() -> Option<Vec<DisplayOutput>> {
    let state = STATE.get_or_init(|| {
        let (c, root) = connect()?;
        let state: State = Arc::new(Mutex::new(query(&c, root)));
        let shared = state.clone();
        std::thread::Builder::new()
            .name("nirubar-randr".into())
            .spawn(move || watch(c, root, shared))
            .ok()?;
        Some(state)
    });
    match state {
        Some(state) => state.lock().ok()?.clone(),
        None => crate::status::display_outputs_xrandr(),
    }
}
