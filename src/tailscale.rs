//! Minimal Tailscale status probe for the bar and launcher.
//!
//! The status comes from tailscaled's LocalAPI on its Unix socket, the same
//! request `tailscale status --json` makes, so a sample is one local HTTP
//! round trip and no process. Without the socket the CLI is asked instead.
//! Either way the probe has a deadline, so a half-started daemon can never
//! block a redraw.

use serde_json::Value;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub host: String,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub backend: String,
    pub peers: Vec<Peer>,
}

const TIMEOUT: Duration = Duration::from_secs(2);
const CAP: usize = 4 * 1024 * 1024;

/// tailscaled's LocalAPI socket. `NIRUBAR_TAILSCALE_SOCKET` points the tests
/// at a fake daemon.
fn socket() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NIRUBAR_TAILSCALE_SOCKET") {
        return Some(PathBuf::from(path));
    }
    [
        "/run/tailscale/tailscaled.sock",
        "/var/run/tailscale/tailscaled.sock",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.exists())
}

fn cli_installed() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("tailscale").is_file()))
}

/// The body of a complete HTTP/1.x response, decoding chunked transfer.
fn http_body(response: &[u8]) -> Option<Vec<u8>> {
    let split = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&response[..split]).ok()?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?;
    if status.split_whitespace().nth(1) != Some("200") {
        return None;
    }
    let chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        })
    });
    let mut body = &response[split + 4..];
    if !chunked {
        return Some(body.to_vec());
    }
    let mut out = Vec::new();
    loop {
        let end = body.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&body[..end]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        body = &body[end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(body.get(..size)?);
        body = body.get(size + 2..)?;
    }
}

/// `GET /localapi/v0/status` over the daemon's socket.
fn local_status(path: &std::path::Path) -> Option<Vec<u8>> {
    let mut stream = std::os::unix::net::UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    stream
        .write_all(
            b"GET /localapi/v0/status HTTP/1.0\r\nHost: local-tailscaled.sock\r\nSec-Tailscale: localapi\r\n\r\n",
        )
        .ok()?;
    let mut response = Vec::new();
    stream
        .take(CAP as u64 + 1)
        .read_to_end(&mut response)
        .ok()?;
    if response.len() > CAP {
        return None;
    }
    http_body(&response)
}

/// Whether Tailscale is here at all: its daemon's socket or its CLI. Only
/// file checks, no process.
pub fn installed() -> bool {
    socket().is_some() || cli_installed()
}

pub fn snapshot() -> Option<Snapshot> {
    if let Some(path) = socket() {
        if let Some(body) = local_status(&path) {
            return parse(&body);
        }
    }
    if !cli_installed() {
        return None;
    }
    let out = crate::probe::run("tailscale", &["status", "--json"], &[], TIMEOUT, 512 * 1024)?;
    if !out.status.success() {
        return None;
    }
    parse(&out.stdout)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

pub fn parse(bytes: &[u8]) -> Option<Snapshot> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let backend = value
        .get("BackendState")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let mut peers = Vec::new();
    if let Some(object) = value.get("Peer").and_then(Value::as_object) {
        for peer in object.values() {
            if !peer.get("Online").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let host = string_field(peer, "DNSName")
                .map(|name| name.trim_end_matches('.').to_string())
                .filter(|name| !name.is_empty())
                .or_else(|| string_field(peer, "HostName"))
                .unwrap_or_else(|| "unknown".into());
            let address = peer
                .get("TailscaleIPs")
                .and_then(Value::as_array)
                .and_then(|ips| ips.first())
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            peers.push(Peer { host, address });
        }
    }
    peers.sort_by(|a, b| a.host.cmp(&b.host));
    Some(Snapshot { backend, peers })
}

pub fn text(snapshot: Option<&Snapshot>, icon: &str, use_icons: bool) -> String {
    let Some(snapshot) = snapshot else {
        return String::new();
    };
    let count = snapshot.peers.len();
    let glyph = if use_icons { icon } else { "ts" };
    if count == 0 {
        glyph.to_string()
    } else {
        format!("{glyph} {count}")
    }
}

pub fn detail(snapshot: &Snapshot) -> Vec<String> {
    let mut lines = vec![
        format!("backend: {}", snapshot.backend),
        format!("{} online peer(s)", snapshot.peers.len()),
    ];
    if !snapshot.peers.is_empty() {
        lines.push(String::new());
        for peer in &snapshot.peers {
            if peer.address.is_empty() {
                lines.push(peer.host.clone());
            } else {
                lines.push(format!("{}  {}", peer.host, peer.address));
            }
        }
    }
    lines.push(String::new());
    lines.push("click: open Tailscale machines in the launcher".into());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peers_are_online_only_and_sorted() {
        let body = br#"{
          "BackendState":"Running",
          "Peer": {
            "b": {"HostName":"beta","DNSName":"beta.example.ts.net.","Online":true,"TailscaleIPs":["100.64.0.2"]},
            "a": {"HostName":"alpha","DNSName":"alpha.example.ts.net.","Online":true,"TailscaleIPs":["100.64.0.1"]},
            "off": {"HostName":"offline","Online":false}
          }
        }"#;
        let snapshot = parse(body).unwrap();
        assert_eq!(snapshot.backend, "Running");
        assert_eq!(
            snapshot.peers,
            vec![
                Peer {
                    host: "alpha.example.ts.net".into(),
                    address: "100.64.0.1".into()
                },
                Peer {
                    host: "beta.example.ts.net".into(),
                    address: "100.64.0.2".into()
                },
            ]
        );
    }

    #[test]
    fn http_bodies_plain_and_chunked() {
        assert_eq!(
            http_body(b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"a\":1}")
                .unwrap(),
            b"{\"a\":1}"
        );
        assert_eq!(
            http_body(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n")
                .unwrap(),
            b"{\"a\":1}"
        );
        assert!(http_body(b"HTTP/1.0 403 Forbidden\r\n\r\nno").is_none());
        assert!(http_body(b"HTTP/1.0 200 OK\r\n").is_none());
    }

    #[test]
    fn the_local_api_answers_over_the_socket() {
        let dir = std::env::temp_dir().join(format!("nirubar-ts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tailscaled.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 512];
            let n = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_string();
            stream
                .write_all(b"HTTP/1.0 200 OK\r\n\r\n{\"BackendState\":\"Running\",\"Peer\":{}}")
                .unwrap();
            request
        });
        let body = local_status(&path).unwrap();
        let request = server.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(request.starts_with("GET /localapi/v0/status HTTP/1.0\r\n"));
        assert!(request.contains("Host: local-tailscaled.sock\r\n"));
        assert_eq!(parse(&body).unwrap().backend, "Running");
    }

    #[test]
    fn text_and_detail_are_silent_without_a_snapshot() {
        assert_eq!(text(None, "T", true), "");
        let snapshot = Snapshot {
            backend: "Stopped".into(),
            peers: vec![],
        };
        assert_eq!(text(Some(&snapshot), "T", true), "T");
        assert!(detail(&snapshot)
            .iter()
            .any(|line| line.contains("Stopped")));
    }
}
