# NIRUBAR patches to system-tray 0.8.9

This is system-tray 0.8.9 by Jake Stanger (MIT licence,
https://github.com/jakestanger/system-tray), vendored by NIRUBAR through
`[patch.crates-io]` in the top-level `Cargo.toml`. Changes:

- The client no longer serves its own `StatusNotifierWatcher`. NIRUBAR runs
  the watcher on a separate connection and thread (`src/sni_watcher.rs`), so
  a slow tray item can never stop the watcher from answering.
- `Client::new` sets a 2 second method timeout (`CALL_TIMEOUT`), and
  `Client::with_connection` accepts a prepared connection.
- Every registered item is set up in its own task, so one item that is slow
  to answer delays only its own icon.
- An item's change signals are drained in their own task and coalesced, and
  the item listens only for its own `NameOwnerChanged`. Previously a pending
  property fetch left an unfiltered `NameOwnerChanged` stream unread; once
  its 64-message queue was full, zbus stopped reading the socket and the
  whole connection froze.
- An item is watched once even when a registration signal races the initial
  item list, and its menu task ends with it.
