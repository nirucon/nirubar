//! One PipeWire reader for all bars on a display.
//!
//! NIRUWM starts one bar process per monitor. Each used to run its own
//! `pw-dump --monitor` and parse the same graph. Now the bars elect a leader
//! through an X selection (`_NIRUBAR_SAMPLER_S<screen>`): the owner runs the
//! hub's worker and publishes every new snapshot as JSON in the root property
//! `_NIRUBAR_AUDIO`; the other bars read that property when it changes
//! (they already listen to root property changes) and run nothing.
//!
//! XFixes tells the followers when the leader's window goes away (a crash, a
//! restart, a monitor unplugged), and the first follower to take the
//! selection leads from then on. Two bars that start at the same time cannot
//! both lead: each looks at the owner and takes the selection while holding
//! the server, so only the first finds it free.

use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{self, ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{Atom, ConnectionExt as _, PropMode, SelectionClearEvent, Window};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

use crate::privacy::{Hub, Snapshot};

pub struct Share {
    selection: Atom,
    property: Atom,
    utf8: Atom,
    root: Window,
    win: Window,
    leader: bool,
    published: Option<String>,
}

/// Larger than any snapshot; a bigger property is not ours.
const MAX_BYTES: u32 = 256 * 1024;

impl Share {
    /// Join the election. Must run before the modules create the hub, so a
    /// follower never starts its own `pw-dump`.
    pub fn new(c: &impl Connection, screen: usize, root: Window, win: Window) -> Option<Self> {
        if std::env::var_os("NIRUBAR_NO_SHARE").is_some() {
            return None;
        }
        let intern = |name: &str| -> Option<Atom> {
            Some(
                c.intern_atom(false, name.as_bytes())
                    .ok()?
                    .reply()
                    .ok()?
                    .atom,
            )
        };
        let selection = intern(&format!("_NIRUBAR_SAMPLER_S{screen}"))?;
        let property = intern("_NIRUBAR_AUDIO")?;
        let utf8 = intern("UTF8_STRING")?;
        // Without XFixes a follower would never notice that the leader left;
        // every bar then reads PipeWire itself, as before.
        c.xfixes_query_version(5, 0).ok()?.reply().ok()?;
        c.xfixes_select_selection_input(
            win,
            selection,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )
        .ok()?;
        let mut share = Self {
            selection,
            property,
            utf8,
            root,
            win,
            leader: false,
            published: None,
        };
        share.claim(c);
        Some(share)
    }

    /// Lead when nobody does, otherwise follow the current leader.
    fn claim(&mut self, c: &impl Connection) {
        // X has no compare-and-set for a selection: two bars that both see
        // no owner would both take it, and both start a reader before one of
        // them hears SelectionClear. Holding the server for the look and the
        // take makes the election atomic; it is two round trips, once.
        let _ = c.grab_server();
        let current = c
            .get_selection_owner(self.selection)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.owner);
        let mut leads = current == Some(self.win);
        if current == Some(x11rb::NONE) {
            let _ = c.set_selection_owner(self.win, self.selection, x11rb::CURRENT_TIME);
            leads = c
                .get_selection_owner(self.selection)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|reply| reply.owner == self.win);
        }
        let _ = c.ungrab_server();
        let _ = c.flush();
        if leads {
            if !self.leader {
                eprintln!("nirubar: reading PipeWire for the bars on this display");
            }
            self.leader = true;
            self.published = None;
            Hub::lead();
        } else {
            self.leader = false;
            Hub::follow();
            self.read(c);
        }
    }

    /// Take the snapshot the leader published.
    fn read(&self, c: &impl Connection) {
        let Some(reply) = c
            .get_property(false, self.root, self.property, self.utf8, 0, MAX_BYTES / 4)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            return;
        };
        if let Ok(snapshot) = serde_json::from_slice::<Snapshot>(&reply.value) {
            Hub::receive(snapshot);
        }
    }

    /// The leader writes a changed snapshot; called before each paint.
    pub fn publish(&mut self, c: &impl Connection) {
        if !self.leader {
            return;
        }
        let Some(hub) = Hub::current() else {
            return;
        };
        let Ok(text) = serde_json::to_string(&hub.snapshot()) else {
            return;
        };
        if self.published.as_deref() == Some(text.as_str()) {
            return;
        }
        if c.change_property8(
            PropMode::REPLACE,
            self.root,
            self.property,
            self.utf8,
            text.as_bytes(),
        )
        .is_ok()
        {
            let _ = c.flush();
            self.published = Some(text);
        }
    }

    /// Handle an event that belongs to the election. True when it did.
    pub fn handle(&mut self, c: &impl Connection, event: &Event) -> bool {
        match event {
            Event::XfixesSelectionNotify(e) if e.selection == self.selection => {
                if e.owner == x11rb::NONE
                    || matches!(
                        e.subtype,
                        xfixes::SelectionEvent::SELECTION_WINDOW_DESTROY
                            | xfixes::SelectionEvent::SELECTION_CLIENT_CLOSE
                    )
                {
                    self.claim(c);
                }
                true
            }
            Event::SelectionClear(SelectionClearEvent { selection, .. })
                if *selection == self.selection =>
            {
                self.leader = false;
                Hub::follow();
                self.read(c);
                true
            }
            Event::PropertyNotify(e) if e.window == self.root && e.atom == self.property => {
                if !self.leader {
                    self.read(c);
                }
                true
            }
            _ => false,
        }
    }
}
