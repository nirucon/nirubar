//! `org.kde.StatusNotifierWatcher`, the registry tray items announce
//! themselves to.
//!
//! The watcher runs on its own thread, Tokio runtime and D-Bus connection,
//! apart from the host in `sni.rs` that talks to the items. Its method and
//! property handlers only touch an in-memory registry and never call an item,
//! so an application that hangs or vanishes cannot stop it from answering.
//! The only outgoing call is a name lookup at the bus daemon, bounded by a
//! short timeout. Items and hosts are dropped as soon as their connection
//! leaves the bus.

use futures_lite::StreamExt;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{mpsc as std_mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::{BusName, UniqueName};
use zbus::object_server::{Interface, SignalEmitter};
use zbus::zvariant::{ObjectPath, Value};
use zbus::Connection;

pub const WATCHER_BUS: &str = "org.kde.StatusNotifierWatcher";
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
/// The object path the specification assumes when a client sends a bus name.
pub const ITEM_PATH: &str = "/StatusNotifierItem";
/// Bound for the watcher's own calls, which only go to the bus daemon.
const CALL_TIMEOUT: Duration = Duration::from_millis(500);
/// How long the host waits for the watcher before it starts anyway.
const START_TIMEOUT: Duration = Duration::from_secs(3);

/// The bus name part of a registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Name {
    /// A connection name such as `:1.42`.
    Unique(String),
    /// A name such as `org.kde.StatusNotifierItem-1234-1`, which must be
    /// resolved to its owner.
    WellKnown(String),
}

/// What a `RegisterStatusNotifierItem` argument points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub name: Name,
    pub path: String,
}

/// Decode the `service` argument of a registration.
///
/// The specification sends a bus name, but clients also send only an object
/// path (libappindicator) or a bus name followed by a path. A path alone
/// belongs to the sender.
pub fn parse_service(service: &str, sender: Option<&str>) -> Result<Target, String> {
    let service = service.trim();
    let (name, path) = if service.starts_with('/') {
        let sender = sender.ok_or("a path-only registration needs a sender")?;
        (sender, service)
    } else if let Some(index) = service.find('/') {
        service.split_at(index)
    } else {
        (service, ITEM_PATH)
    };
    ObjectPath::try_from(path).map_err(|error| format!("invalid object path {path:?}: {error}"))?;
    let name = match BusName::try_from(name)
        .map_err(|error| format!("invalid bus name {name:?}: {error}"))?
    {
        BusName::Unique(unique) => Name::Unique(unique.to_string()),
        BusName::WellKnown(known) => Name::WellKnown(known.to_string()),
    };
    Ok(Target {
        name,
        path: path.to_string(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    owner: String,
    path: String,
    /// The well-known name the item registered with, if any.
    alias: Option<String>,
}

impl Entry {
    fn address(&self) -> String {
        format!("{}{}", self.owner, self.path)
    }
}

/// Result of adding an item.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Added {
    /// The new address, or `None` when the item was already registered.
    pub address: Option<String>,
    /// Older entries this registration replaced.
    pub replaced: Vec<String>,
}

/// What leaving the bus removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Removed {
    pub items: Vec<String>,
    pub host: bool,
}

/// The watcher's bookkeeping, kept free of D-Bus so it can be tested.
#[derive(Debug, Default)]
pub struct Registry {
    items: Vec<Entry>,
    hosts: Vec<String>,
}

impl Registry {
    /// Add an item owned by the unique name `owner`. A second registration of
    /// the same owner and path is a duplicate. A well-known name has one owner
    /// at a time, so an entry another connection registered under the same
    /// name and path (an application restarted before the old instance left
    /// the bus) is replaced.
    pub fn add_item(&mut self, owner: &str, path: &str, alias: Option<&str>) -> Added {
        if self
            .items
            .iter()
            .any(|entry| entry.owner == owner && entry.path == path)
        {
            return Added::default();
        }
        let mut replaced = Vec::new();
        if let Some(alias) = alias {
            self.items.retain(|entry| {
                let stale = entry.owner != owner
                    && entry.path == path
                    && entry.alias.as_deref() == Some(alias);
                if stale {
                    replaced.push(entry.address());
                }
                !stale
            });
        }
        let entry = Entry {
            owner: owner.to_string(),
            path: path.to_string(),
            alias: alias.map(str::to_string),
        };
        let address = entry.address();
        self.items.push(entry);
        Added {
            address: Some(address),
            replaced,
        }
    }

    /// Remove the items of `owner`: the one at `path`, or all of them.
    pub fn remove_items(&mut self, owner: &str, path: Option<&str>) -> Vec<String> {
        let mut removed = Vec::new();
        self.items.retain(|entry| {
            let gone = entry.owner == owner && path.map_or(true, |path| entry.path == path);
            if gone {
                removed.push(entry.address());
            }
            !gone
        });
        removed
    }

    /// Forget everything a connection that left the bus had registered.
    pub fn remove_owner(&mut self, owner: &str) -> Removed {
        let items = self.remove_items(owner, None);
        let before = self.hosts.len();
        self.hosts.retain(|host| host != owner);
        Removed {
            items,
            host: self.hosts.len() != before,
        }
    }

    /// Add a host. False when it was already registered.
    pub fn add_host(&mut self, owner: &str) -> bool {
        if self.hosts.iter().any(|host| host == owner) {
            return false;
        }
        self.hosts.push(owner.to_string());
        true
    }

    /// Registered item addresses in registration order.
    pub fn items(&self) -> Vec<String> {
        self.items.iter().map(Entry::address).collect()
    }

    pub fn has_host(&self) -> bool {
        !self.hosts.is_empty()
    }

    pub fn host_count(&self) -> usize {
        self.hosts.len()
    }
}

type Shared = Arc<Mutex<Registry>>;

fn with<R>(registry: &Shared, f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut guard = match registry.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

struct Watcher {
    registry: Shared,
    dbus: DBusProxy<'static>,
}

impl Watcher {
    /// Turn a target into (unique owner, path, well-known alias).
    async fn resolve(
        &self,
        target: Target,
        sender: Option<&str>,
    ) -> zbus::fdo::Result<(String, String, Option<String>)> {
        match target.name {
            Name::Unique(owner) => Ok((owner, target.path, None)),
            Name::WellKnown(name) => {
                let bus_name = BusName::try_from(name.as_str())
                    .map_err(|error| zbus::fdo::Error::InvalidArgs(error.to_string()))?;
                match tokio::time::timeout(CALL_TIMEOUT, self.dbus.get_name_owner(bus_name)).await {
                    Ok(Ok(owner)) => Ok((owner.to_string(), target.path, Some(name))),
                    Ok(Err(error)) => Err(error),
                    // The daemon is slow: the sender is by far the most likely
                    // owner, and a wrong guess is dropped when it leaves.
                    Err(_) => sender
                        .map(|sender| (sender.to_string(), target.path, Some(name)))
                        .ok_or_else(|| zbus::fdo::Error::Failed("name lookup timed out".into())),
                }
            }
        }
    }

    /// Drop `owner` again if it left the bus before the watcher noticed, so
    /// a registration racing its sender's exit cannot leave a ghost.
    fn verify_owner(&self, owner: String, emitter: SignalEmitter<'static>) {
        let dbus = self.dbus.clone();
        let registry = self.registry.clone();
        tokio::spawn(async move {
            let Ok(name) = UniqueName::try_from(owner.as_str()) else {
                return;
            };
            let check = dbus.name_has_owner(BusName::Unique(name));
            if let Ok(Ok(false)) = tokio::time::timeout(CALL_TIMEOUT, check).await {
                drop_owner(&registry, &emitter, &owner).await;
            }
        });
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(ToString::to_string);
        let target =
            parse_service(service, sender.as_deref()).map_err(zbus::fdo::Error::InvalidArgs)?;
        let (owner, path, alias) = self.resolve(target, sender.as_deref()).await?;
        let added = with(&self.registry, |registry| {
            registry.add_item(&owner, &path, alias.as_deref())
        });
        let Some(address) = added.address else {
            return Ok(());
        };
        for old in &added.replaced {
            let _ = Self::status_notifier_item_unregistered(&emitter, old).await;
        }
        announce_items(&emitter, &self.registry).await;
        let _ = Self::status_notifier_item_registered(&emitter, &address).await;
        self.verify_owner(owner, emitter.to_owned());
        Ok(())
    }

    /// Not in the specification, but sent by some hosts when an item leaves.
    async fn unregister_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(ToString::to_string);
        let explicit_path = service.trim().contains('/');
        let target =
            parse_service(service, sender.as_deref()).map_err(zbus::fdo::Error::InvalidArgs)?;
        let Ok((owner, path, _)) = self.resolve(target, sender.as_deref()).await else {
            return Ok(());
        };
        let removed = with(&self.registry, |registry| {
            registry.remove_items(&owner, explicit_path.then_some(path.as_str()))
        });
        if removed.is_empty() {
            return Ok(());
        }
        for address in &removed {
            let _ = Self::status_notifier_item_unregistered(&emitter, address).await;
        }
        announce_items(&emitter, &self.registry).await;
        Ok(())
    }

    async fn register_status_notifier_host(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(ToString::to_string);
        let target =
            parse_service(service, sender.as_deref()).map_err(zbus::fdo::Error::InvalidArgs)?;
        let (owner, _, _) = self.resolve(target, sender.as_deref()).await?;
        let (added, first) = with(&self.registry, |registry| {
            let added = registry.add_host(&owner);
            (added, registry.host_count() == 1)
        });
        if !added {
            return Ok(());
        }
        if first {
            announce_host(&emitter, true).await;
        }
        let _ = Self::status_notifier_host_registered(&emitter).await;
        self.verify_owner(owner, emitter.to_owned());
        Ok(())
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        with(&self.registry, |registry| registry.items())
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        with(&self.registry, |registry| registry.has_host())
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_unregistered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

async fn properties_changed(emitter: &SignalEmitter<'_>, name: &str, value: Value<'_>) {
    let mut changed = HashMap::new();
    changed.insert(name, value);
    if let Err(error) = zbus::fdo::Properties::properties_changed(
        emitter,
        Watcher::name(),
        changed,
        Cow::Borrowed(&[]),
    )
    .await
    {
        eprintln!("nirubar: sni watcher: cannot announce {name}: {error}");
    }
}

async fn announce_items(emitter: &SignalEmitter<'_>, registry: &Shared) {
    let items = with(registry, |registry| registry.items());
    properties_changed(emitter, "RegisteredStatusNotifierItems", Value::from(items)).await;
}

async fn announce_host(emitter: &SignalEmitter<'_>, registered: bool) {
    properties_changed(
        emitter,
        "IsStatusNotifierHostRegistered",
        Value::from(registered),
    )
    .await;
}

/// Forget what `owner` registered and tell the hosts.
async fn drop_owner(registry: &Shared, emitter: &SignalEmitter<'_>, owner: &str) {
    let (removed, hosts_left) = with(registry, |registry| {
        (registry.remove_owner(owner), registry.has_host())
    });
    for address in &removed.items {
        let _ = Watcher::status_notifier_item_unregistered(emitter, address).await;
    }
    if !removed.items.is_empty() {
        announce_items(emitter, registry).await;
    }
    if removed.host {
        if !hosts_left {
            announce_host(emitter, false).await;
        }
        let _ = Watcher::status_notifier_host_unregistered(emitter).await;
    }
}

/// A watcher serving on a connection.
pub struct Running {
    /// Kept so the connection lives as long as the watcher.
    #[allow(dead_code)]
    pub connection: Connection,
    /// True when this watcher owns `org.kde.StatusNotifierWatcher`; false
    /// when another one does and this one waits in the queue.
    pub primary: bool,
}

/// Serve the watcher on the bus `builder` connects to.
///
/// The `NameOwnerChanged` stream is subscribed before the name is requested
/// and is read by a task that does nothing else, so its queue cannot fill.
pub async fn serve(builder: zbus::connection::Builder<'_>) -> zbus::Result<Running> {
    let connection = builder.method_timeout(CALL_TIMEOUT * 2).build().await?;
    let dbus = DBusProxy::new(&connection).await?;
    let mut owners = dbus.receive_name_owner_changed().await?;
    let registry = Shared::default();
    connection
        .object_server()
        .at(
            WATCHER_PATH,
            Watcher {
                registry: registry.clone(),
                dbus: dbus.clone(),
            },
        )
        .await?;

    let emitter = SignalEmitter::new(&connection, WATCHER_PATH)?.into_owned();
    tokio::spawn(async move {
        while let Some(signal) = owners.next().await {
            let Ok(args) = signal.args() else {
                continue;
            };
            // Only a connection leaving the bus matters; a released
            // well-known name leaves the item reachable by its owner.
            if args.new_owner().is_some() || !args.name().starts_with(':') {
                continue;
            }
            let owner = args.name().to_string();
            drop_owner(&registry, &emitter, &owner).await;
        }
    });

    // Queue behind a running watcher instead of failing; it takes over
    // when that one exits.
    let flags: [RequestNameFlags; 0] = [];
    let primary = match connection
        .request_name_with_flags(WATCHER_BUS, flags.into_iter().collect())
        .await
    {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => true,
        Ok(_) | Err(zbus::Error::NameTaken) => false,
        Err(error) => return Err(error),
    };
    Ok(Running {
        connection,
        primary,
    })
}

/// Start the session watcher once per process, waiting a little for it to
/// own its name so the host registers with it rather than with nobody.
pub fn ensure_running() -> Result<bool, String> {
    static STARTED: OnceLock<Result<bool, String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            let (ready, wait) = std_mpsc::channel();
            thread::Builder::new()
                .name("nirubar-sniw".into())
                .spawn(move || run(ready))
                .map_err(|error| format!("cannot start the watcher thread: {error}"))?;
            match wait.recv_timeout(START_TIMEOUT) {
                Ok(result) => result,
                Err(_) => Err("the watcher did not start in time".into()),
            }
        })
        .clone()
}

fn run(ready: std_mpsc::Sender<Result<bool, String>>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(format!("cannot start Tokio runtime: {error}")));
            return;
        }
    };
    runtime.block_on(async move {
        let builder = match zbus::connection::Builder::session() {
            Ok(builder) => builder,
            Err(error) => {
                let _ = ready.send(Err(format!("session bus unavailable: {error}")));
                return;
            }
        };
        match serve(builder).await {
            Ok(running) => {
                let _ = ready.send(Ok(running.primary));
                // The object server and the owner task run on this runtime.
                std::future::pending::<()>().await;
            }
            Err(error) => {
                let _ = ready.send(Err(format!("session bus unavailable: {error}")));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bus_name_defaults_to_the_item_path() {
        let target = parse_service("org.kde.StatusNotifierItem-12-1", Some(":1.9")).unwrap();
        assert_eq!(
            target,
            Target {
                name: Name::WellKnown("org.kde.StatusNotifierItem-12-1".into()),
                path: ITEM_PATH.into(),
            }
        );
        let target = parse_service(":1.42", Some(":1.9")).unwrap();
        assert_eq!(target.name, Name::Unique(":1.42".into()));
        assert_eq!(target.path, ITEM_PATH);
    }

    #[test]
    fn a_path_alone_belongs_to_the_sender() {
        let target =
            parse_service("/org/ayatana/NotificationItem/nextcloud", Some(":1.7")).unwrap();
        assert_eq!(target.name, Name::Unique(":1.7".into()));
        assert_eq!(target.path, "/org/ayatana/NotificationItem/nextcloud");
        assert!(parse_service("/StatusNotifierItem", None).is_err());
    }

    #[test]
    fn a_name_with_a_path_is_split() {
        let target = parse_service(":1.72/org/ayatana/NotificationItem/dropbox", None).unwrap();
        assert_eq!(target.name, Name::Unique(":1.72".into()));
        assert_eq!(target.path, "/org/ayatana/NotificationItem/dropbox");
    }

    #[test]
    fn malformed_services_are_rejected() {
        for service in ["", "not a name", ":1.5/bad//path", "/trailing/"] {
            assert!(
                parse_service(service, Some(":1.1")).is_err(),
                "{service:?} should be rejected"
            );
        }
    }

    #[test]
    fn registration_ignores_duplicates_and_keeps_order() {
        let mut registry = Registry::default();
        let first = registry.add_item(":1.5", ITEM_PATH, None);
        assert_eq!(first.address.as_deref(), Some(":1.5/StatusNotifierItem"));
        assert_eq!(registry.add_item(":1.5", ITEM_PATH, None), Added::default());
        registry.add_item(":1.6", "/org/ayatana/NotificationItem/app", None);
        assert_eq!(
            registry.items(),
            [
                ":1.5/StatusNotifierItem",
                ":1.6/org/ayatana/NotificationItem/app"
            ]
        );
    }

    #[test]
    fn leaving_the_bus_drops_items_and_host() {
        let mut registry = Registry::default();
        registry.add_item(":1.5", ITEM_PATH, None);
        registry.add_item(":1.5", "/second", None);
        registry.add_item(":1.6", ITEM_PATH, None);
        assert!(registry.add_host(":1.5"));
        assert!(!registry.add_host(":1.5"));
        let removed = registry.remove_owner(":1.5");
        assert_eq!(
            removed,
            Removed {
                items: vec![":1.5/StatusNotifierItem".into(), ":1.5/second".into()],
                host: true,
            }
        );
        assert_eq!(registry.items(), [":1.6/StatusNotifierItem"]);
        assert!(!registry.has_host());
        assert_eq!(registry.remove_owner(":1.99"), Removed::default());
    }

    #[test]
    fn a_restarted_application_replaces_its_old_entry() {
        // Two instances briefly coexist; the new one takes the well-known
        // name, so the old registration under that name is stale.
        let mut registry = Registry::default();
        registry.add_item(":1.5", ITEM_PATH, Some("org.kde.StatusNotifierItem-100-1"));
        registry.add_item(":1.8", ITEM_PATH, Some("org.kde.StatusNotifierItem-200-1"));
        let added = registry.add_item(":1.9", ITEM_PATH, Some("org.kde.StatusNotifierItem-100-1"));
        assert_eq!(added.address.as_deref(), Some(":1.9/StatusNotifierItem"));
        assert_eq!(added.replaced, [":1.5/StatusNotifierItem"]);
        assert_eq!(
            registry.items(),
            [":1.8/StatusNotifierItem", ":1.9/StatusNotifierItem"]
        );
        // The old instance exiting later removes nothing that is still live.
        assert!(registry.remove_owner(":1.5").items.is_empty());
    }

    #[test]
    fn unregistering_removes_one_path_or_all_of_an_owner() {
        let mut registry = Registry::default();
        registry.add_item(":1.5", ITEM_PATH, None);
        registry.add_item(":1.5", "/second", None);
        assert_eq!(
            registry.remove_items(":1.5", Some("/second")),
            [":1.5/second"]
        );
        assert_eq!(
            registry.remove_items(":1.5", None),
            [":1.5/StatusNotifierItem"]
        );
        assert!(registry.items().is_empty());
    }

    /// End-to-end checks on a private `dbus-daemon`. They never touch the
    /// user's session bus: the daemon gets its own socket directory, HOME and
    /// XDG directories, and every connection is made to its address.
    mod live {
        use super::super::*;
        use std::io::{BufRead, BufReader};
        use std::path::PathBuf;
        use std::process::{Child, Command, Stdio};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Instant;
        use system_tray::client::Client;
        use zbus::names::InterfaceName;

        struct Bus {
            child: Child,
            address: String,
            dir: PathBuf,
        }

        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn private_bus() -> Option<Bus> {
            use std::os::unix::fs::PermissionsExt;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            // A short path under /tmp: a Unix socket path must stay below
            // 108 bytes, which a long TMPDIR can exceed.
            let dir = PathBuf::from("/tmp").join(format!(
                "nsw-{}-{}",
                std::process::id(),
                stamp % 1_000_000_000
            ));
            for sub in ["home", "runtime", "config", "data", "socket"] {
                std::fs::create_dir_all(dir.join(sub)).ok()?;
                let _ =
                    std::fs::set_permissions(dir.join(sub), std::fs::Permissions::from_mode(0o700));
            }
            let config = dir.join("bus.conf");
            std::fs::write(
                &config,
                format!(
                    "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \
                     \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n\
                     <busconfig><type>session</type>\
                     <listen>unix:dir={}</listen><auth>EXTERNAL</auth>\
                     <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
                     <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>\n",
                    dir.join("socket").display()
                ),
            )
            .ok()?;
            let mut child = match Command::new("dbus-daemon")
                .arg(format!("--config-file={}", config.display()))
                .args(["--nofork", "--print-address=1"])
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", dir.join("home"))
                .env("XDG_RUNTIME_DIR", dir.join("runtime"))
                .env("XDG_CONFIG_HOME", dir.join("config"))
                .env("XDG_DATA_HOME", dir.join("data"))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) => {
                    eprintln!("skipping: cannot start dbus-daemon: {error}");
                    let _ = std::fs::remove_dir_all(&dir);
                    return None;
                }
            };
            let mut line = String::new();
            let stdout = child.stdout.take()?;
            BufReader::new(stdout).read_line(&mut line).ok()?;
            let address = line.trim().to_string();
            let bus = Bus {
                child,
                address,
                dir,
            };
            (!bus.address.is_empty()).then_some(bus)
        }

        /// A tray item whose Title getter can be told to hang forever.
        struct FakeItem {
            stuck: Arc<AtomicBool>,
            title: Arc<Mutex<String>>,
        }

        #[zbus::interface(name = "org.kde.StatusNotifierItem")]
        impl FakeItem {
            #[zbus(property)]
            fn id(&self) -> String {
                "fake".into()
            }

            #[zbus(property)]
            fn status(&self) -> String {
                "Active".into()
            }

            #[zbus(property)]
            async fn title(&self) -> String {
                if self.stuck.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
                self.title.lock().map(|t| t.clone()).unwrap_or_default()
            }
        }

        struct Fake {
            connection: Connection,
            stuck: Arc<AtomicBool>,
            title: Arc<Mutex<String>>,
        }

        async fn connect(address: &str) -> Connection {
            zbus::connection::Builder::address(address)
                .unwrap()
                .build()
                .await
                .unwrap()
        }

        async fn fake_item(address: &str, title: &str, known: Option<&str>) -> Fake {
            let stuck = Arc::new(AtomicBool::new(false));
            let title = Arc::new(Mutex::new(title.to_string()));
            let connection = zbus::connection::Builder::address(address)
                .unwrap()
                .serve_at(
                    ITEM_PATH,
                    FakeItem {
                        stuck: stuck.clone(),
                        title: title.clone(),
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let service = match known {
                Some(name) => {
                    connection.request_name(name).await.unwrap();
                    name.to_string()
                }
                None => ITEM_PATH.to_string(),
            };
            connection
                .call_method(
                    Some(WATCHER_BUS),
                    WATCHER_PATH,
                    Some(WATCHER_BUS),
                    "RegisterStatusNotifierItem",
                    &(service.as_str(),),
                )
                .await
                .unwrap();
            Fake {
                connection,
                stuck,
                title,
            }
        }

        async fn emit_new_title(fake: &Fake) {
            fake.connection
                .emit_signal(
                    None::<()>,
                    ITEM_PATH,
                    "org.kde.StatusNotifierItem",
                    "NewTitle",
                    &(),
                )
                .await
                .unwrap();
        }

        /// Ask the watcher for its items on a fresh connection, timing it.
        async fn registered(address: &str) -> (Vec<String>, Duration) {
            let connection = connect(address).await;
            let start = Instant::now();
            let proxy = zbus::fdo::PropertiesProxy::builder(&connection)
                .destination(WATCHER_BUS)
                .unwrap()
                .path(WATCHER_PATH)
                .unwrap()
                .build()
                .await
                .unwrap();
            let value = tokio::time::timeout(
                Duration::from_secs(1),
                proxy.get(
                    InterfaceName::from_static_str_unchecked(WATCHER_BUS),
                    "RegisteredStatusNotifierItems",
                ),
            )
            .await
            .expect("the watcher answers within a second")
            .unwrap();
            let items = Vec::<String>::try_from(value).unwrap();
            (items, start.elapsed())
        }

        async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !check() {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        fn host_title(client: &Client, owner: &str) -> Option<String> {
            let items = client.items();
            let map = items.lock().unwrap();
            map.get(owner)
                .map(|(item, _)| item.title.clone().unwrap_or_default())
        }

        #[test]
        fn a_hung_item_cannot_stall_the_watcher_or_the_host() {
            let Some(bus) = private_bus() else {
                return;
            };
            let address = bus.address.clone();

            // The watcher on its own thread and runtime, as in the bar.
            let (ready_tx, ready_rx) = std_mpsc::channel();
            let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
            let watcher_address = address.clone();
            let watcher = thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    let builder =
                        zbus::connection::Builder::address(watcher_address.as_str()).unwrap();
                    let running = serve(builder).await.unwrap();
                    ready_tx.send(running.primary).unwrap();
                    let _ = tokio::task::spawn_blocking(move || stop_rx.recv()).await;
                    drop(running);
                });
            });
            assert!(ready_rx.recv_timeout(Duration::from_secs(5)).unwrap());

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let host_connection = zbus::connection::Builder::address(address.as_str())
                    .unwrap()
                    .method_timeout(system_tray::client::CALL_TIMEOUT)
                    .build()
                    .await
                    .unwrap();
                let client = Client::with_connection(host_connection).await.unwrap();

                let good = fake_item(&address, "Good", None).await;
                let stuck =
                    fake_item(&address, "Sync", Some("org.kde.StatusNotifierItem-4242-1")).await;
                let good_owner = good.connection.unique_name().unwrap().to_string();
                let stuck_owner = stuck.connection.unique_name().unwrap().to_string();
                eventually("both items in the host", || {
                    host_title(&client, &good_owner).is_some()
                        && host_title(&client, &stuck_owner).is_some()
                })
                .await;

                // The stuck item announces a change, then never answers the
                // fetch. Meanwhile many clients come and go, which used to
                // fill the host's NameOwnerChanged queue and freeze the
                // connection the watcher shared.
                stuck.stuck.store(true, Ordering::SeqCst);
                emit_new_title(&stuck).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                for _ in 0..150 {
                    drop(connect(&address).await);
                }

                let (items, took) = registered(&address).await;
                assert!(took < Duration::from_secs(1), "watcher took {took:?}");
                assert_eq!(
                    items,
                    [
                        format!("{good_owner}{ITEM_PATH}"),
                        format!("{stuck_owner}{ITEM_PATH}")
                    ]
                );

                // The host still follows the healthy item.
                *good.title.lock().unwrap() = "Good again".into();
                emit_new_title(&good).await;
                eventually("the healthy item's new title", || {
                    host_title(&client, &good_owner).as_deref() == Some("Good again")
                })
                .await;

                // The stuck application is killed: both sides drop it. Its
                // hung getter keeps a clone alive, so close the socket the way
                // a killed process does.
                let _ = stuck.connection.clone().close().await;
                drop(stuck);
                let start = Instant::now();
                loop {
                    let (items, _) = registered(&address).await;
                    if items == [format!("{good_owner}{ITEM_PATH}")] {
                        break;
                    }
                    assert!(
                        start.elapsed() < Duration::from_secs(1),
                        "stale items {items:?}"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                eventually("the host to drop the killed item", || {
                    host_title(&client, &stuck_owner).is_none()
                })
                .await;

                // A new instance registers under the same well-known name.
                let again =
                    fake_item(&address, "Sync", Some("org.kde.StatusNotifierItem-4242-1")).await;
                let again_owner = again.connection.unique_name().unwrap().to_string();
                let (items, _) = registered(&address).await;
                assert_eq!(
                    items,
                    [
                        format!("{good_owner}{ITEM_PATH}"),
                        format!("{again_owner}{ITEM_PATH}")
                    ]
                );
                eventually("the host to show the new instance", || {
                    host_title(&client, &again_owner).as_deref() == Some("Sync")
                })
                .await;
            });
            drop(runtime);
            let _ = stop_tx.send(());
            let _ = watcher.join();
            drop(bus);
        }
    }
}
