//! A small, bounded reader for D-Bus object managers on the system bus.
//!
//! `drives` (UDisks2) and `headset` (BlueZ) need the same thing: every object
//! a service exports, with the few properties the bar shows. One call to
//! `GetManagedObjects` returns all of them, so a module samples a whole
//! service in a single round trip on its own worker thread and never spawns a
//! helper. The values are reduced to [`Prop`], which the tests and the test
//! fixtures can build without a bus.

use std::collections::BTreeMap;
use std::time::Duration;

/// A property value, reduced to what the modules read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prop {
    Bool(bool),
    Text(String),
    Number(u64),
    /// Arrays of byte strings (`aay`), such as UDisks2 mount points.
    Bytes(Vec<Vec<u8>>),
    /// Any other array, by its length (UDisks2 `Configuration`).
    Count(usize),
    Other,
}

/// Interface name to property name to value.
pub type Interfaces = BTreeMap<String, BTreeMap<String, Prop>>;
/// Object path to its interfaces.
pub type Objects = BTreeMap<String, Interfaces>;

/// The value of `iface.prop` on one object.
pub fn prop<'a>(object: &'a Interfaces, iface: &str, name: &str) -> Option<&'a Prop> {
    object.get(iface)?.get(name)
}

pub fn flag(object: &Interfaces, iface: &str, name: &str) -> bool {
    matches!(prop(object, iface, name), Some(Prop::Bool(true)))
}

pub fn text<'a>(object: &'a Interfaces, iface: &str, name: &str) -> &'a str {
    match prop(object, iface, name) {
        Some(Prop::Text(t)) => t,
        _ => "",
    }
}

pub fn number(object: &Interfaces, iface: &str, name: &str) -> Option<u64> {
    match prop(object, iface, name) {
        Some(Prop::Number(n)) => Some(*n),
        _ => None,
    }
}

fn reduce(value: &zbus::zvariant::Value<'_>) -> Prop {
    use zbus::zvariant::Value;
    match value {
        Value::Bool(b) => Prop::Bool(*b),
        Value::Str(s) => Prop::Text(s.to_string()),
        Value::ObjectPath(p) => Prop::Text(p.to_string()),
        Value::U8(n) => Prop::Number(u64::from(*n)),
        Value::U16(n) => Prop::Number(u64::from(*n)),
        Value::U32(n) => Prop::Number(u64::from(*n)),
        Value::U64(n) => Prop::Number(*n),
        Value::I32(n) if *n >= 0 => Prop::Number(*n as u64),
        Value::I64(n) if *n >= 0 => Prop::Number(*n as u64),
        Value::Value(inner) => reduce(inner),
        Value::Array(items) => {
            // `ay`: one byte string (a UDisks2 device path).
            if items.iter().all(|i| matches!(i, Value::U8(_))) {
                let one = items
                    .iter()
                    .filter_map(|i| match i {
                        Value::U8(b) => Some(*b),
                        _ => None,
                    })
                    .collect();
                return Prop::Bytes(vec![one]);
            }
            let mut out = Vec::new();
            for item in items.iter() {
                let Value::Array(bytes) = item else {
                    return Prop::Count(items.len());
                };
                let mut one = Vec::new();
                for b in bytes.iter() {
                    match b {
                        Value::U8(b) => one.push(*b),
                        _ => return Prop::Count(items.len()),
                    }
                }
                out.push(one);
            }
            Prop::Bytes(out)
        }
        _ => Prop::Other,
    }
}

/// A system-bus connection kept between samples.
pub struct Reader {
    runtime: Option<tokio::runtime::Runtime>,
    connection: Option<zbus::Connection>,
    /// The service's signals, subscribed once per connection.
    signals: Option<zbus::MessageStream>,
}

impl Reader {
    pub fn new() -> Self {
        Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok(),
            connection: None,
            signals: None,
        }
    }

    /// Sleep until `service` sends a signal under `path` (an object added or
    /// removed, a property changed), `max` passed, or `stop` was notified.
    /// A burst of signals is answered once. False when stopped.
    pub fn wait(
        &mut self,
        service: &str,
        path: &str,
        max: Duration,
        stop: &tokio::sync::Notify,
    ) -> bool {
        use futures_lite::StreamExt;
        let Some(runtime) = self.runtime.as_ref() else {
            return false;
        };
        if self.signals.is_none() {
            if let Some(connection) = self.connection.as_ref() {
                self.signals = runtime.block_on(async {
                    let rule = zbus::MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .sender(service)
                        .ok()?
                        .path_namespace(path)
                        .ok()?
                        .build();
                    tokio::time::timeout(
                        Duration::from_secs(2),
                        zbus::MessageStream::for_match_rule(rule, connection, Some(64)),
                    )
                    .await
                    .ok()?
                    .ok()
                });
            }
        }
        let signals = &mut self.signals;
        let (stopped, lost) = runtime.block_on(async {
            let Some(stream) = signals.as_mut() else {
                // No bus yet: wait for the next try (or the stop).
                tokio::select! {
                    _ = stop.notified() => return (true, false),
                    _ = tokio::time::sleep(max) => return (false, false),
                }
            };
            tokio::select! {
                _ = stop.notified() => (true, false),
                _ = tokio::time::sleep(max) => (false, false),
                next = stream.next() => {
                    if next.is_none() {
                        return (false, true);
                    }
                    // Let the rest of a burst arrive, then sample once.
                    let settle = tokio::time::sleep(Duration::from_millis(200));
                    tokio::pin!(settle);
                    loop {
                        tokio::select! {
                            _ = stop.notified() => return (true, false),
                            _ = &mut settle => return (false, false),
                            more = stream.next() => if more.is_none() { return (false, true) },
                        }
                    }
                }
            }
        });
        if lost {
            self.signals = None;
            self.connection = None;
        }
        !stopped
    }

    /// Every object `service` exports under `path`, or `None` when the
    /// service (or the bus) does not answer within two seconds.
    pub fn objects(&mut self, service: &str, path: &str) -> Option<Objects> {
        let runtime = self.runtime.as_ref()?;
        if self.connection.is_none() {
            let connection = runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(2), zbus::Connection::system()).await
                })
                .ok()?
                .ok()?;
            self.connection = Some(connection);
        }
        let connection = self.connection.as_ref()?;
        let managed = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), async {
                let proxy = zbus::fdo::ObjectManagerProxy::builder(connection)
                    .destination(service)
                    .ok()?
                    .path(path)
                    .ok()?
                    .cache_properties(zbus::proxy::CacheProperties::No)
                    .build()
                    .await
                    .ok()?;
                proxy.get_managed_objects().await.ok()
            })
            .await
        });
        let Ok(Some(managed)) = managed else {
            // A restarted bus or service: connect again next time.
            self.connection = None;
            self.signals = None;
            return None;
        };
        let mut out = Objects::new();
        for (object, interfaces) in managed {
            let mut ifaces = Interfaces::new();
            for (iface, props) in interfaces {
                let props = props
                    .iter()
                    .map(|(name, value)| (name.clone(), reduce(value)))
                    .collect();
                ifaces.insert(iface.to_string(), props);
            }
            out.insert(object.to_string(), ifaces);
        }
        Some(out)
    }
}

/// A sample of a D-Bus service that is taken again when the service sends a
/// signal, and at the latest after `max`, on one long-lived worker thread.
/// Idle, the worker sleeps in the bus socket: nothing is polled.
pub struct Watched<T> {
    value: std::sync::Arc<std::sync::Mutex<Option<T>>>,
    stop: std::sync::Arc<tokio::sync::Notify>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl<T> Watched<T>
where
    T: Clone + PartialEq + Send + 'static,
{
    pub fn new(
        service: &'static str,
        path: &'static str,
        max: Duration,
        mut sample: impl FnMut(&mut Reader) -> Option<T> + Send + 'static,
    ) -> Self {
        let value = std::sync::Arc::new(std::sync::Mutex::new(None));
        let stop = std::sync::Arc::new(tokio::sync::Notify::new());
        let (shared, stopping) = (value.clone(), stop.clone());
        let worker = std::thread::Builder::new()
            .name("nirubar-dbus".into())
            .spawn(move || {
                let mut reader = Reader::new();
                loop {
                    let next = sample(&mut reader);
                    if let Ok(mut current) = shared.lock() {
                        if *current != next {
                            *current = next;
                            drop(current);
                            crate::wake_update();
                        }
                    }
                    if !reader.wait(service, path, max, &stopping) {
                        return;
                    }
                }
            })
            .ok();
        Self {
            value,
            stop,
            worker,
        }
    }

    pub fn get(&self) -> Option<T> {
        self.value.lock().ok().and_then(|value| value.clone())
    }
}

impl<T> Drop for Watched<T> {
    fn drop(&mut self) {
        // notify_one keeps a permit, so a worker that is sampling right now
        // stops at its next wait.
        self.stop.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Objects from a test fixture: `path<TAB>interface<TAB>property<TAB>value`
/// lines, where the value is `b:true`, `n:42`, `s:text`, `c:2` (an array of that length) or `y:/a;/b` (byte
/// strings separated by `;`). Used by the smoke tests instead of a bus.
pub fn parse_fixture(text: &str) -> Objects {
    let mut out = Objects::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.splitn(4, '\t').collect();
        if fields.len() != 4 {
            continue;
        }
        let value = match fields[3].split_once(':') {
            Some(("b", v)) => Prop::Bool(v == "true"),
            Some(("n", v)) => v.parse().map(Prop::Number).unwrap_or(Prop::Other),
            Some(("s", v)) => Prop::Text(v.into()),
            Some(("c", v)) => v.parse().map(Prop::Count).unwrap_or(Prop::Other),
            Some(("y", v)) => Prop::Bytes(
                v.split(';')
                    .filter(|p| !p.is_empty())
                    .map(|p| p.as_bytes().to_vec())
                    .collect(),
            ),
            _ => Prop::Other,
        };
        out.entry(fields[0].into())
            .or_default()
            .entry(fields[1].into())
            .or_default()
            .insert(fields[2].into(), value);
    }
    out
}

/// Fixture objects when `var` names a readable file.
pub fn fixture(var: &str) -> Option<Objects> {
    let path = std::env::var_os(var)?;
    Some(parse_fixture(
        &std::fs::read_to_string(path).unwrap_or_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_build_the_same_shapes_as_the_bus() {
        let o = parse_fixture(
            "/a\torg.X\tOn\tb:true\n/a\torg.X\tName\ts:Stick\n/a\torg.Y\tSize\tn:16\n/a\torg.Y\tMounts\ty:/m/a;/m/b\nbad line\n",
        );
        let a = &o["/a"];
        assert!(flag(a, "org.X", "On"));
        assert_eq!(text(a, "org.X", "Name"), "Stick");
        assert_eq!(number(a, "org.Y", "Size"), Some(16));
        assert_eq!(
            prop(a, "org.Y", "Mounts"),
            Some(&Prop::Bytes(vec![b"/m/a".to_vec(), b"/m/b".to_vec()]))
        );
        assert_eq!(o.len(), 1);
    }
}
