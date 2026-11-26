#[cfg(feature = "data")]
use crate::data::apply_menu_diffs;
use crate::data::TrayItemMap;
use crate::dbus::dbus_menu_proxy::{DBusMenuProxy, PropertiesUpdate};
use crate::dbus::notifier_item_proxy::StatusNotifierItemProxy;
use crate::dbus::notifier_watcher_proxy::StatusNotifierWatcherProxy;
use crate::dbus::{self, OwnedValueExt};
use crate::error::{Error, Result};
use crate::item::{self, IconPixmap, Status, StatusNotifierItem, Tooltip};
use crate::menu::{MenuDiff, TrayMenu};
use crate::names;
use dbus::DBusProps;
use futures_lite::StreamExt;
use std::collections::HashSet;
use std::future::Future;
use std::result;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::spawn;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{sleep, timeout, Instant};
use tracing::{debug, error, trace, warn};
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::InterfaceName;
use zbus::zvariant::{Array, Structure, Value};
use zbus::Connection;

use self::names::ITEM_OBJECT;

/// An event emitted by the client
/// representing a change from either the `StatusNotifierItem`
/// or `DBusMenu` protocols.
#[derive(Debug, Clone)]
pub enum Event {
    /// A new `StatusNotifierItem` was added.
    Add(String, Box<StatusNotifierItem>),
    /// An update was received for an existing `StatusNotifierItem`.
    /// This could be either an update to the item itself,
    /// or an update to the associated menu.
    Update(String, UpdateEvent),
    /// A `StatusNotifierItem` was unregistered.
    Remove(String),
}

/// The specific change associated with an update event.
#[derive(Debug, Clone)]
pub enum UpdateEvent {
    AttentionIcon(Option<String>),
    Icon {
        icon_name: Option<String>,
        icon_pixmap: Option<Vec<IconPixmap>>,
    },
    OverlayIcon(Option<String>),
    Status(Status),
    Title(Option<String>),
    Tooltip(Option<Tooltip>),
    /// A menu layout has changed.
    /// The entire layout is sent.
    Menu(TrayMenu),
    /// One or more menu properties have changed.
    /// Only the updated properties are sent.
    MenuDiff(Vec<MenuDiff>),
    /// A new menu has connected to the item.
    /// Its name on bus is sent.
    MenuConnect(String),
}

/// A request to 'activate' one of the menu items,
/// typically sent when it is clicked.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ActivateRequest {
    /// Submenu ID
    MenuItem {
        address: String,
        menu_path: String,
        submenu_id: i32,
    },
    /// Default activation for the tray.
    /// The parameter(x and y) represents screen coordinates and is to be considered an hint to the item where to show eventual windows (if any).
    Default { address: String, x: i32, y: i32 },
    /// Secondary activation(less important) for the tray.
    /// The parameter(x and y) represents screen coordinates and is to be considered an hint to the item where to show eventual windows (if any).
    Secondary { address: String, x: i32, y: i32 },
}

const PROPERTIES_INTERFACE: &str = "org.kde.StatusNotifierItem";

/// Upper bound for every method call the host makes (NIRUBAR patch).
///
/// zbus has no default call timeout. A tray item that is alive but never
/// answers would otherwise park a task forever, and while that task is parked
/// any signal stream it owns stops being read. zbus queues at most 64 messages
/// per stream and then stops reading the socket, so one stuck item used to
/// freeze the whole connection.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// Addresses that currently have a watcher task, so a registration signal
/// racing the initial item list does not start a second one.
type Watched = Arc<Mutex<HashSet<String>>>;
const ITEM_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(200),
    Duration::from_millis(750),
];

async fn retry_with_delays<T, E, I, F, Fut, N>(
    delays: I,
    mut operation: F,
    mut on_retry: N,
) -> std::result::Result<T, E>
where
    I: IntoIterator<Item = Duration>,
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, E>>,
    N: FnMut(&E, Duration),
{
    let mut delays = delays.into_iter();

    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => match delays.next() {
                Some(delay) => {
                    on_retry(&error, delay);
                    sleep(delay).await;
                }
                None => return Err(error),
            },
        }
    }
}

/// Claim `address` for one watcher task. False when one already runs.
fn claim(watched: &Watched, address: &str) -> bool {
    watched
        .lock()
        .map(|mut set| set.insert(address.to_string()))
        .unwrap_or(false)
}

fn release(watched: &Watched, address: &str) {
    if let Ok(mut set) = watched.lock() {
        set.remove(address);
    }
}

/// Add `member` to the pending updates unless it is already queued. Property
/// fetches read the current value, so one fetch serves any number of
/// identical change signals.
fn coalesce(pending: &mut Vec<String>, member: String) {
    if !pending.contains(&member) {
        pending.push(member);
    }
}

/// Client for watching the tray.
#[derive(Debug)]
pub struct Client {
    tx: broadcast::Sender<Event>,
    _rx: broadcast::Receiver<Event>,
    connection: Connection,

    #[cfg(feature = "data")]
    items: TrayItemMap,
}

impl Client {
    /// Creates and initializes the client.
    ///
    /// The client will begin listening to items and menus and sending events immediately.
    /// It is recommended that consumers immediately follow the call to `new` with a `subscribe` call,
    /// then immediately follow that with a call to `items` to get the state to not miss any events.
    ///
    /// The value of `service_name` must be unique on the session bus.
    /// It is recommended to use something similar to the format of `appid-numid`,
    /// where `numid` is a short-ish random integer.
    ///
    /// # Errors
    ///
    /// If the initialization fails for any reason,
    /// for example if unable to connect to the bus,
    /// this method will return an error.
    ///
    /// # Panics
    ///
    /// If the generated well-known name is invalid, the library will panic
    /// as this indicates a major bug.
    ///
    /// Likewise, the spawned tasks may panic if they cannot get a `Mutex` lock.
    pub async fn new() -> Result<Self> {
        let connection = zbus::connection::Builder::session()?
            .method_timeout(CALL_TIMEOUT)
            .build()
            .await?;
        Self::with_connection(connection).await
    }

    /// Start a host on an existing connection (NIRUBAR patch).
    ///
    /// Unlike upstream, the client does not serve a `StatusNotifierWatcher`
    /// on its own connection: the watcher must answer even while the host is
    /// waiting on a slow item, so the embedding program runs it on a
    /// separate connection. The connection should carry a method timeout,
    /// as [`Client::new`] sets.
    ///
    /// # Errors
    ///
    /// Fails when the host cannot request its name or reach the watcher.
    pub async fn with_connection(connection: Connection) -> Result<Self> {
        let (tx, rx) = broadcast::channel(32);
        let watched: Watched = Arc::default();

        let watcher_proxy = StatusNotifierWatcherProxy::new(&connection).await?;

        // register a host on the watcher to declare we want to watch items
        // get a well-known name
        let pid = std::process::id();
        let mut i = 0;
        let wellknown = loop {
            use zbus::fdo::RequestNameReply::{AlreadyOwner, Exists, InQueue, PrimaryOwner};

            i += 1;
            let wellknown = format!("org.freedesktop.StatusNotifierHost-{pid}-{i}");
            let wellknown: zbus::names::WellKnownName = wellknown
                .try_into()
                .expect("generated well-known name is invalid");

            let flags = [zbus::fdo::RequestNameFlags::DoNotQueue];
            match connection
                .request_name_with_flags(&wellknown, flags.into_iter().collect())
                .await?
            {
                PrimaryOwner => break wellknown,
                Exists | AlreadyOwner => {}
                InQueue => unreachable!(
                    "request_name_with_flags returned InQueue even though we specified DoNotQueue"
                ),
            };
        };

        debug!("wellknown: {wellknown}");
        watcher_proxy
            .register_status_notifier_host(&wellknown)
            .await?;
        let items = TrayItemMap::new();

        // handle new items
        {
            let connection = connection.clone();
            let tx = tx.clone();
            let items = items.clone();

            let watched = watched.clone();

            let mut stream = watcher_proxy
                .receive_status_notifier_item_registered()
                .await?;

            // Each registration gets its own task: the stream is read again at
            // once, and an item that is slow to answer delays only itself.
            spawn(async move {
                while let Some(item) = stream.next().await {
                    let address = match item.args() {
                        Ok(args) => args.service.to_string(),
                        Err(error) => {
                            error!("failed to parse tray item registration: {error}");
                            continue;
                        }
                    };
                    debug!("received new item: {address}");
                    spawn(Self::start_item(
                        address,
                        connection.clone(),
                        tx.clone(),
                        items.clone(),
                        watched.clone(),
                    ));
                }
            });
        }

        // then lastly get all items
        // it can take so long to fetch all items that we have to do this last,
        // otherwise some incoming items get missed
        {
            let connection = connection.clone();
            let tx = tx.clone();
            let items = items.clone();
            let watched = watched.clone();

            spawn(async move {
                let initial_items = watcher_proxy.registered_status_notifier_items().await?;
                debug!("initial items: {initial_items:?}");

                for item in initial_items {
                    spawn(Self::start_item(
                        item,
                        connection.clone(),
                        tx.clone(),
                        items.clone(),
                        watched.clone(),
                    ));
                }

                Ok::<(), Error>(())
            });
        }

        // Handle other watchers unregistering and this one taking over
        // It is necessary to clear all items as our watcher will then re-send them all
        {
            let tx = tx.clone();
            let items = items.clone();

            let dbus_proxy = DBusProxy::new(&connection).await?;

            let mut stream = dbus_proxy.receive_name_acquired().await?;

            spawn(async move {
                while let Some(thing) = stream.next().await {
                    let body = thing.args()?;
                    if body.name == names::WATCHER_BUS {
                        for dest in items.clear_items() {
                            tx.send(Event::Remove(dest))?;
                        }
                    }
                }

                Ok::<(), Error>(())
            });
        }

        debug!("tray client initialized");

        Ok(Self {
            connection,
            tx,
            _rx: rx,
            #[cfg(feature = "data")]
            items,
        })
    }

    /// Watch one item unless a task already does, logging a failure.
    async fn start_item(
        address: String,
        connection: Connection,
        tx: broadcast::Sender<Event>,
        items: TrayItemMap,
        watched: Watched,
    ) {
        if !claim(&watched, &address) {
            debug!("tray item {address} is already watched");
            return;
        }
        if let Err(error) =
            Self::handle_item(&address, connection, tx, items, watched.clone()).await
        {
            error!("failed to initialize tray item {address}: {error}");
            release(&watched, &address);
        }
    }

    /// Processes an incoming item to send the initial add event,
    /// then set up listeners for it and its menu.
    async fn handle_item(
        address: &str,
        connection: Connection,
        tx: broadcast::Sender<Event>,
        items: TrayItemMap,
        watched: Watched,
    ) -> Result<()> {
        let (destination, path) = parse_address(address);

        let properties_proxy = PropertiesProxy::builder(&connection)
            .destination(destination.to_string())?
            .path(path.clone())?
            .build()
            .await?;

        let properties = Self::get_item_properties(destination, &path, &properties_proxy).await?;

        items.new_item(destination.into(), &properties);

        tx.send(Event::Add(
            destination.to_string(),
            properties.clone().into(),
        ))?;

        let menu_task = if let Some(menu) = properties.menu {
            let destination = destination.to_string();
            let connection = connection.clone();
            let items = items.clone();
            let tx = tx.clone();

            tx.send(Event::Update(
                destination.clone(),
                UpdateEvent::MenuConnect(menu.clone()),
            ))?;

            Some(spawn(async move {
                if let Err(error) =
                    Self::watch_menu(destination.clone(), &menu, &connection, tx, items).await
                {
                    warn!("stopped watching the menu of {destination}: {error}");
                }
            }))
        } else {
            None
        };

        {
            let destination = destination.to_string();
            let address = address.to_string();

            spawn(async move {
                if let Err(error) = Self::watch_item_properties(
                    &destination,
                    &path,
                    &connection,
                    properties_proxy,
                    tx,
                    items,
                )
                .await
                {
                    warn!("stopped watching {destination}{path}: {error}");
                }
                // The menu belongs to the item; it goes with it.
                if let Some(menu_task) = menu_task {
                    menu_task.abort();
                }
                release(&watched, &address);
                debug!("Stopped watching {destination}{path}");
            });
        }

        Ok(())
    }

    /// Gets the properties for an SNI item.
    async fn get_item_properties(
        destination: &str,
        path: &str,
        properties_proxy: &PropertiesProxy<'_>,
    ) -> Result<StatusNotifierItem> {
        let properties = retry_with_delays(
            ITEM_RETRY_DELAYS,
            || {
                properties_proxy.get_all(
                    InterfaceName::from_static_str(PROPERTIES_INTERFACE)
                        .expect("to be valid interface name"),
                )
            },
            |error, delay| {
                warn!(
                    "failed to read tray item {destination}{path}: {error}; retrying in {} ms",
                    delay.as_millis()
                );
            },
        )
        .await?;

        StatusNotifierItem::try_from(DBusProps(properties))
    }

    /// Watches an SNI item's properties,
    /// sending an update event whenever they change.
    async fn watch_item_properties(
        destination: &str,
        path: &str,
        connection: &Connection,
        properties_proxy: PropertiesProxy<'_>,
        tx: broadcast::Sender<Event>,
        items: TrayItemMap,
    ) -> Result<()> {
        let notifier_item_proxy = StatusNotifierItemProxy::builder(connection)
            .destination(destination)?
            .path(path)?
            .build()
            .await?;

        let dbus_proxy = DBusProxy::new(connection).await?;

        // Only this item's own name changes, not every client on the bus: an
        // unfiltered stream fills while a property fetch is in flight.
        let mut disconnect_stream = dbus_proxy
            .receive_name_owner_changed_with_args(&[(0, destination)])
            .await?;
        let mut props_changed = notifier_item_proxy.inner().receive_all_signals().await?;

        // Read change signals in their own task, so the stream is drained even
        // while a property fetch waits on a slow item.
        let (member_tx, mut member_rx) = mpsc::unbounded_channel::<String>();
        let drain = spawn(async move {
            while let Some(change) = props_changed.next().await {
                let Some(member) = change.header().member().map(ToString::to_string) else {
                    continue;
                };
                if member_tx.send(member).is_err() {
                    break;
                }
            }
        });

        // The item may have left before the stream above was subscribed.
        let gone_already = matches!(
            dbus_proxy
                .name_has_owner(destination.try_into().map_err(zbus::Error::from)?)
                .await,
            Ok(false)
        );

        let result: Result<bool> = 'watch: loop {
            if gone_already {
                break Ok(true);
            }
            tokio::select! {
                Some(member) = member_rx.recv() => {
                    let mut pending = vec![member];
                    while let Ok(more) = member_rx.try_recv() {
                        coalesce(&mut pending, more);
                    }
                    for member in pending {
                        match Self::get_update_event(&member, &properties_proxy).await {
                            Ok(Some(event)) => {
                                debug!("[{destination}{path}] received property change: {event:?}");

                                cfg_if::cfg_if! {
                                    if #[cfg(feature = "data")] {
                                        items.apply_update_event(destination, &event);
                                    }
                                }

                                if tx.send(Event::Update(destination.to_string(), event)).is_err() {
                                    break 'watch Ok(false);
                                }
                            }
                            Err(e) => {
                                error!("Error parsing update properties from {destination}{path}: {e:?}");
                            }
                            _ => {}
                        }
                    }
                }
                Some(signal) = disconnect_stream.next() => {
                    let Ok(args) = signal.args() else { continue };
                    if args.new_owner().is_none() {
                        break Ok(true);
                    }
                }
                else => break Ok(false),
            }
        };
        drain.abort();

        if let Ok(true) = result {
            debug!("[{destination}{path}] disconnected");

            if let Ok(watcher_proxy) = StatusNotifierWatcherProxy::new(connection).await {
                if let Err(error) = watcher_proxy
                    .unregister_status_notifier_item(destination)
                    .await
                {
                    debug!("{error:?}");
                }
            }

            items.remove_item(destination);

            tx.send(Event::Remove(destination.to_string()))?;
        }
        result.map(|_| ())
    }

    /// Gets the update event for a `DBus` properties change signal.
    async fn get_update_event(
        member: &str,
        properties_proxy: &PropertiesProxy<'_>,
    ) -> Result<Option<UpdateEvent>> {
        use UpdateEvent::{AttentionIcon, Icon, OverlayIcon, Status, Title, Tooltip};

        macro_rules! get_property {
            ($name:expr) => {
                match properties_proxy
                    .get(
                        InterfaceName::from_static_str(PROPERTIES_INTERFACE)
                            .expect("to be valid interface name"),
                        $name,
                    )
                    .await
                {
                    Ok(v) => Ok(Some(v)),
                    Err(e) => match e {
                        // Some properties may not be set, and this error will be raised.
                        zbus::fdo::Error::InvalidArgs(_) => {
                            warn!("{e}");
                            Ok(None)
                        }
                        _ => Err(Into::<Error>::into(e)),
                    },
                }
            };
        }

        let property = match member {
            "NewAttentionIcon" => Some(AttentionIcon(
                get_property!("AttentionIconName")?
                    .as_ref()
                    .map(OwnedValueExt::to_string)
                    .transpose()?,
            )),
            "NewIcon" => {
                let icon_name = get_property!("IconName")
                    .unwrap_or_else(|e| {
                        warn!("Error getting IconName: {e:?}");
                        None
                    })
                    .as_ref()
                    .map(OwnedValueExt::to_string)
                    .transpose()
                    .ok()
                    .flatten();

                let icon_pixmap = get_property!("IconPixmap")
                    .unwrap_or_else(|e| {
                        warn!("Error getting IconPixmap: {e:?}");
                        None
                    })
                    .as_deref()
                    .map(Value::downcast_ref::<&Array>)
                    .and_then(result::Result::ok)
                    .map(IconPixmap::from_array)
                    .and_then(result::Result::ok);

                Some(Icon {
                    icon_name,
                    icon_pixmap,
                })
            }
            "NewOverlayIcon" => Some(OverlayIcon(
                get_property!("OverlayIconName")?
                    .as_ref()
                    .map(OwnedValueExt::to_string)
                    .transpose()?,
            )),
            "NewStatus" => Some(Status(
                get_property!("Status")?
                    .as_deref()
                    .map(Value::downcast_ref::<&str>)
                    .transpose()?
                    .map(item::Status::from)
                    .unwrap_or_default(), // NOTE: i'm assuming status is always set
            )),
            "NewTitle" => Some(Title(
                get_property!("Title")?
                    .as_ref()
                    .map(OwnedValueExt::to_string)
                    .transpose()?,
            )),
            "NewToolTip" => Some(Tooltip(
                get_property!("ToolTip")?
                    .as_deref()
                    .map(Value::downcast_ref::<&Structure>)
                    .transpose()?
                    .map(crate::item::Tooltip::try_from)
                    .transpose()?,
            )),
            _ => {
                warn!("received unhandled update event: {member}");
                None
            }
        };

        debug!("received tray item update: {member} -> {property:?}");

        Ok(property)
    }

    /// Watches the `DBusMenu` associated with an SNI item.
    ///
    /// This gets the initial menu, sending an update event immediately.
    /// Update events are then sent for any further updates
    /// until the item is removed.
    async fn watch_menu(
        destination: String,
        menu_path: &str,
        connection: &Connection,
        tx: broadcast::Sender<Event>,
        items: TrayItemMap,
    ) -> Result<()> {
        const LAYOUT_UPDATE_INTERVAL_MS: Duration = Duration::from_millis(50);

        let dbus_menu_proxy = DBusMenuProxy::builder(connection)
            .destination(destination.as_str())?
            .path(menu_path)?
            .build()
            .await?;

        debug!("[{destination}{menu_path}] getting initial menu");
        let menu = dbus_menu_proxy.get_layout(0, -1, &[]).await?;
        let menu = TrayMenu::try_from(menu)?;

        items.update_menu(&destination, &menu);

        tx.send(Event::Update(destination.clone(), UpdateEvent::Menu(menu)))?;

        let mut layout_updated = dbus_menu_proxy.receive_layout_updated().await?;
        let mut properties_updated = dbus_menu_proxy.receive_items_properties_updated().await?;

        let last_layout_update = Arc::new(Mutex::new(Instant::now()));
        let (layout_tx, mut layout_rx) = mpsc::channel(4);

        loop {
            tokio::select!(
                Some(ev) = layout_updated.next() => {
                    trace!("received layout update");

                    let now = Instant::now();
                    *last_layout_update.lock().expect("should get lock") = now;

                    let args = ev.args()?;

                    let last_layout_update = last_layout_update.clone();
                    let layout_tx = layout_tx.clone();
                    spawn(async move {
                        sleep(LAYOUT_UPDATE_INTERVAL_MS).await;
                        if *last_layout_update.lock().expect("should get lock") == now {
                            trace!("dispatching layout update");
                            let _ = layout_tx.send(args.parent).await;
                        }
                    });
                }
                Some(layout_parent) = layout_rx.recv() => {
                    debug!("[{destination}{menu_path}] layout update");

                    let get_layout = dbus_menu_proxy.get_layout(layout_parent, -1, &[]);

                    let menu = match timeout(Duration::from_secs(1), get_layout).await {
                        Ok(Ok(menu)) => {
                            debug!("got new menu layout");
                            menu
                        }
                        Ok(Err(err)) => {
                            error!("error fetching layout: {err:?}");
                            break;
                        }
                        Err(_) => {
                            error!("Timeout getting layout");
                            break;
                        }
                    };

                    let menu = TrayMenu::try_from(menu)?;

                    items.update_menu(&destination, &menu);

                    debug!("sending new menu for '{destination}'");
                    trace!("new menu for '{destination}': {menu:?}");
                    tx.send(Event::Update(
                        destination.clone(),
                        UpdateEvent::Menu(menu),
                    ))?;
                }
                Some(change) = properties_updated.next() => {
                    let body = change.message().body();
                    let update: PropertiesUpdate= body.deserialize::<PropertiesUpdate>()?;
                    let diffs = Vec::try_from(update)?;

                    #[cfg(feature = "data")]
                    if let Some((_, Some(menu))) = items
                        .get_map()
                        .lock()
                        .expect("mutex lock should succeed")
                        .get_mut(&destination)
                    {
                        apply_menu_diffs(menu, &diffs);
                    } else {
                        error!("could not find item in state");
                    }

                    tx.send(Event::Update(
                        destination.clone(),
                        UpdateEvent::MenuDiff(diffs),
                    ))?;

                    // FIXME: Menu cache gonna be out of sync
                }
                else => break,
            );
        }

        Ok(())
    }

    async fn get_notifier_item_proxy(
        &self,
        address: String,
    ) -> Result<StatusNotifierItemProxy<'_>> {
        let proxy = StatusNotifierItemProxy::builder(&self.connection)
            .destination(address)?
            .path(ITEM_OBJECT)?
            .build()
            .await?;
        Ok(proxy)
    }

    async fn get_menu_proxy(
        &self,
        address: String,
        menu_path: String,
    ) -> Result<DBusMenuProxy<'_>> {
        let proxy = DBusMenuProxy::builder(&self.connection)
            .destination(address)?
            .path(menu_path)?
            .build()
            .await?;

        Ok(proxy)
    }

    /// Subscribes to the events broadcast channel,
    /// returning a new receiver.
    ///
    /// Once the client is dropped, the receiver will close.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Gets all current items, including their menus if present.
    #[cfg(feature = "data")]
    #[must_use]
    pub fn items(&self) -> std::sync::Arc<std::sync::Mutex<crate::data::BaseMap>> {
        self.items.get_map()
    }

    /// One should call this method with id=0 when opening the root menu.
    ///
    /// ID refers to the menuitem id.
    /// Returns `needsUpdate`
    ///
    /// # Errors
    ///
    /// Errors if the proxy cannot be created.
    pub async fn about_to_show_menuitem(
        &self,
        address: String,
        menu_path: String,
        id: i32,
    ) -> Result<bool> {
        let proxy = self.get_menu_proxy(address, menu_path).await?;
        Ok(proxy.about_to_show(id).await?)
    }

    /// Sends an activate request for a menu item.
    ///
    /// # Errors
    ///
    /// The method will return an error if the connection to the `DBus` object fails,
    /// or if sending the event fails for any reason.
    ///
    /// # Panics
    ///
    /// If the system time is somehow before the Unix epoch.
    pub async fn activate(&self, req: ActivateRequest) -> Result<()> {
        macro_rules! timeout_event {
            ($event:expr) => {
                if timeout(Duration::from_secs(1), $event).await.is_err() {
                    error!("Timed out sending activate event");
                }
            };
        }
        match req {
            ActivateRequest::MenuItem {
                address,
                menu_path,
                submenu_id,
            } => {
                let proxy = self.get_menu_proxy(address, menu_path).await?;
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time should flow forwards");

                let event = proxy.event(
                    submenu_id,
                    "clicked",
                    &Value::I32(0),
                    timestamp.as_secs() as u32,
                );

                timeout_event!(event);
            }
            ActivateRequest::Default { address, x, y } => {
                let proxy = self.get_notifier_item_proxy(address).await?;
                let event = proxy.activate(x, y);

                timeout_event!(event);
            }
            ActivateRequest::Secondary { address, x, y } => {
                let proxy = self.get_notifier_item_proxy(address).await?;
                let event = proxy.secondary_activate(x, y);

                timeout_event!(event);
            }
        }

        Ok(())
    }
}

fn parse_address(address: &str) -> (&str, String) {
    address
        .split_once('/')
        .map_or((address, String::from("/StatusNotifierItem")), |(d, p)| {
            (d, format!("/{p}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn item_initialization_retries_transient_failures() {
        use std::{cell::Cell, future::ready};

        let attempts = Cell::new(0);
        let result = retry_with_delays(
            [Duration::ZERO; 2],
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                ready((attempt == 3).then_some(attempt).ok_or(attempt))
            },
            |_, _| {},
        )
        .await;

        assert_eq!(result, Ok(3));
        assert_eq!(attempts.get(), 3);
    }

    #[tokio::test]
    async fn item_initialization_returns_last_failure() {
        use std::{cell::Cell, future::ready};

        let attempts = Cell::new(0);
        let result = retry_with_delays(
            [Duration::ZERO; 2],
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                ready(Err::<(), _>(attempt))
            },
            |_, _| {},
        )
        .await;

        assert_eq!(result, Err(3));
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn an_address_is_watched_once_until_released() {
        let watched = Watched::default();
        assert!(claim(&watched, ":1.5/StatusNotifierItem"));
        assert!(!claim(&watched, ":1.5/StatusNotifierItem"));
        assert!(claim(&watched, ":1.6/StatusNotifierItem"));
        release(&watched, ":1.5/StatusNotifierItem");
        assert!(claim(&watched, ":1.5/StatusNotifierItem"));
    }

    #[test]
    fn repeated_change_signals_are_fetched_once() {
        let mut pending = Vec::new();
        for member in ["NewIcon", "NewToolTip", "NewIcon", "NewIcon", "NewStatus"] {
            coalesce(&mut pending, member.to_string());
        }
        assert_eq!(pending, ["NewIcon", "NewToolTip", "NewStatus"]);
    }

    #[test]
    fn parse_unnamed() {
        let address = ":1.58/StatusNotifierItem";
        let (destination, path) = parse_address(address);

        assert_eq!(":1.58", destination);
        assert_eq!("/StatusNotifierItem", path);
    }

    #[test]
    fn parse_named() {
        let address = ":1.72/org/ayatana/NotificationItem/dropbox_client_1398";
        let (destination, path) = parse_address(address);

        assert_eq!(":1.72", destination);
        assert_eq!("/org/ayatana/NotificationItem/dropbox_client_1398", path);
    }
}
