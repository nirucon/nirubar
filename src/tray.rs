//! A system tray, by the XEmbed protocol.
//!
//! Applications that show a tray icon — `nm-applet`, `volumeicon`,
//! `redshift-gtk`, Steam — look for a tray by asking who owns the selection
//! `_NET_SYSTEM_TRAY_S0`. This module owns that selection, so those programs
//! place their icons in the bar.
//!
//! XEmbed is the older sibling of D-Bus StatusNotifierItem, and it is what most
//! small applications still speak. Supporting it costs a selection owner and
//! two messages, and needs no daemon.

use crate::config::Result;
use x11rb::{
    connection::Connection, protocol::xproto::*, rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// How wide one icon is given, in pixels.
pub const ICON_SIZE: u16 = 18;
/// Space between icons.
const GAP: u16 = 2;

/// The tray selection and the messages XEmbed defines.
pub struct Tray {
    /// Our tray window, the selection owner.
    pub win: Window,
    /// The selection atom, so it can be released on exit.
    #[allow(dead_code)] // kept so a future version can re-announce itself
    pub selection: Atom,
    opcode_send: Atom,
    embed_info: Atom,
    /// The XEmbed client-message type (distinct from the info property).
    xembed: Atom,
    /// Icons currently in the tray, left to right.
    pub icons: Vec<Window>,
}

impl Tray {
    /// Announce a tray on screen `screen` and start accepting icons.
    pub fn new(c: &RustConnection, root: Window, screen: usize) -> Result<Self> {
        let atom =
            |n: &str| -> Result<Atom> { Ok(c.intern_atom(false, n.as_bytes())?.reply()?.atom) };
        let selection = atom(&format!("_NET_SYSTEM_TRAY_S{screen}"))?;
        let opcode_send = atom("_NET_SYSTEM_TRAY_OPCODE")?;
        let embed_info = atom("_XEMBED_INFO")?;
        let xembed = atom("_XEMBED")?;
        let orientation = atom("_NET_SYSTEM_TRAY_ORIENTATION")?;
        let visual = atom("_NET_SYSTEM_TRAY_VISUAL")?;
        let tray_manager = atom("MANAGER")?;

        // Another tray already owns the selection: leave it alone.
        if c.get_selection_owner(selection)?.reply()?.owner != 0 {
            return Err("another system tray is already running".into());
        }

        let win = c.generate_id()?;
        c.create_window(
            0,
            win,
            root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                // An application docks by sending a ClientMessage to this
                // window with SubstructureNotifyMask, so that mask must be
                // selected here or the request is dropped by the server.
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::STRUCTURE_NOTIFY
                        | EventMask::SUBSTRUCTURE_NOTIFY,
                ),
        )?;
        // Horizontal, and with the root visual so icons compose normally.
        c.change_property32(
            PropMode::REPLACE,
            win,
            orientation,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        c.change_property32(
            PropMode::REPLACE,
            win,
            visual,
            AtomEnum::VISUALID,
            &[c.setup().roots[screen].root_visual],
        )?;
        // XEmbed clients expect the manager window itself to be mapped; an
        // unmapped 1x1 parent made some icons never appear.
        c.map_window(win)?;
        c.set_selection_owner(win, selection, x11rb::CURRENT_TIME)?;

        // Tell every running application that a tray exists now.
        c.send_event(
            false,
            root,
            EventMask::STRUCTURE_NOTIFY,
            ClientMessageEvent::new(
                32,
                root,
                tray_manager,
                [x11rb::CURRENT_TIME, selection, win, 0, 0],
            ),
        )?;
        c.flush()?;
        Ok(Self {
            win,
            selection,
            opcode_send,
            embed_info,
            xembed,
            icons: Vec::new(),
        })
    }

    /// How much width the icons need.
    pub fn width(&self) -> u16 {
        if self.icons.is_empty() {
            0
        } else {
            self.icons.len() as u16 * (ICON_SIZE + GAP)
        }
    }

    /// Handle a message addressed to the tray. Returns true when it was one.
    pub fn handle(&mut self, c: &RustConnection, e: &ClientMessageEvent) -> bool {
        // XEmbed asks the tray to accept an icon with opcode 0.
        if e.type_ == self.opcode_send {
            let data = e.data.as_data32();
            if data[1] == 0 {
                let icon = data[2];
                self.accept(c, icon);
            }
            return true;
        }
        false
    }

    /// Embed one icon.
    fn accept(&mut self, c: &RustConnection, icon: Window) {
        if self.icons.contains(&icon) {
            return;
        }
        // The icon must be reparented into our window and told so. Without the
        // _XEMBED_INFO property a client stays unembedded and never draws.
        let _ = c.change_save_set(SetMode::INSERT, icon);
        let _ = c.change_window_attributes(
            icon,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
        );
        let _ = c.reparent_window(icon, self.win, 0, 0);
        let _ = c.map_window(icon);
        let _ = c.change_property32(
            PropMode::REPLACE,
            icon,
            self.embed_info,
            self.embed_info,
            &[0, 1],
        );
        // XEMBED_EMBEDDED_NOTIFY (0): data1 is the embedder window.
        let _ = c.send_event(
            false,
            icon,
            EventMask::NO_EVENT,
            ClientMessageEvent::new(
                32,
                icon,
                self.xembed,
                [x11rb::CURRENT_TIME, 0, 0, self.win, 0],
            ),
        );
        self.icons.push(icon);
        let _ = c.flush();
    }

    /// Place the icons from `start_x` leftward, which is where a right-aligned
    /// tray wants them.
    pub fn layout(
        &self,
        c: &RustConnection,
        y: i16,
        right_edge: i16,
        bar_height: u16,
    ) -> Result<()> {
        let mut x = right_edge - self.width() as i16;
        let icon_y = y + ((bar_height as i16 - ICON_SIZE as i16) / 2).max(0);
        for icon in &self.icons {
            c.configure_window(
                *icon,
                &ConfigureWindowAux::new()
                    .x(i32::from(x))
                    .y(i32::from(icon_y))
                    .width(u32::from(ICON_SIZE))
                    .height(u32::from(ICON_SIZE)),
            )?;
            x += (ICON_SIZE + GAP) as i16;
        }
        Ok(())
    }

    /// Forget an icon that went away.
    pub fn remove(&mut self, icon: Window) {
        self.icons.retain(|w| *w != icon);
    }

    /// Release the tray selection.
    pub fn release(&self, c: &RustConnection) {
        let _ = c.set_selection_owner(x11rb::NONE, self.selection, x11rb::CURRENT_TIME);
        let _ = c.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_width_grows_with_icons() {
        let tray = Tray {
            win: 1,
            selection: 2,
            opcode_send: 3,
            embed_info: 5,
            xembed: 6,
            icons: vec![],
        };
        assert_eq!(tray.width(), 0);
        let mut tray = tray;
        tray.icons.push(10);
        assert!(tray.width() >= ICON_SIZE);
        tray.icons.push(11);
        assert!(tray.width() >= ICON_SIZE * 2);
    }

    #[test]
    fn removing_the_last_icon_leaves_no_width() {
        let mut tray = Tray {
            win: 1,
            selection: 2,
            opcode_send: 3,
            embed_info: 5,
            xembed: 6,
            icons: vec![10],
        };
        tray.remove(10);
        assert_eq!(tray.width(), 0);
    }
}
