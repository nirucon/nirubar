//! Minimal Tailscale status probe for the bar and launcher.
//!
//! `tailscale status --json` is local, but it still gets a deadline so a
//! half-started daemon can never block a redraw.

use serde_json::Value;
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

pub fn installed() -> bool {
    crate::probe::run(
        "tailscale",
        &["version"],
        &[],
        Duration::from_secs(2),
        64 * 1024,
    )
    .is_some_and(|out| out.status.success())
}

pub fn snapshot() -> Option<Snapshot> {
    let out = crate::probe::run(
        "tailscale",
        &["status", "--json"],
        &[],
        Duration::from_secs(2),
        512 * 1024,
    )?;
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
