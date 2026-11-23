//! Everything that goes over the bus passes through here: encoding (JSON or compact binary,
//! switchable at runtime from the debug panel), per-kind traffic counters for the metrics
//! graphs, and rate-limited warnings when a message can't be sent.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::warn;

use crate::stats::{self, Counter};

/// Encode new messages as MessagePack (true) or JSON (false). Set from every assignment, so
/// all processes switch together; receivers read either, so the switch never drops anything.
static BINARY: AtomicBool = AtomicBool::new(true);

pub fn set_binary(on: bool) {
    BINARY.store(on, Relaxed);
}

pub fn binary() -> bool {
    BINARY.load(Relaxed)
}

/// What a message is, from its subject, for the "bus traffic by kind" graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    /// Group -> Spatial Layer Cell: entity positions every tick (+ bots' questions, events).
    Reports,
    /// Spatial Layer Cell -> group: answers to bots' questions.
    Senses,
    /// Envelopes between entities, groups and cells: inputs, requests, effects, handoffs.
    Envelopes,
    /// Spatial Layer Cell -> gateway: what each player sees. Gateway -> cells: where players are.
    Views,
    /// Every process -> gateway: the debug bus view's data.
    Debug,
    /// Heartbeats, assignments, commands, stats.
    Control,
}

impl MsgKind {
    pub const ALL: [MsgKind; 6] = [
        MsgKind::Reports,
        MsgKind::Senses,
        MsgKind::Envelopes,
        MsgKind::Views,
        MsgKind::Debug,
        MsgKind::Control,
    ];

    pub fn name(self) -> &'static str {
        match self {
            MsgKind::Reports => "reports",
            MsgKind::Senses => "senses",
            MsgKind::Envelopes => "envelopes",
            MsgKind::Views => "views",
            MsgKind::Debug => "debug",
            MsgKind::Control => "control",
        }
    }

    pub fn of(subject: &str) -> MsgKind {
        if subject.ends_with(".reports") && subject != crate::subjects::DEBUG_REPORTS {
            MsgKind::Reports
        } else if subject.ends_with(".senses") {
            MsgKind::Senses
        } else if subject.ends_with(".inbox") || subject.starts_with("entity.") {
            MsgKind::Envelopes
        } else if subject == crate::subjects::VIEWS || subject == crate::subjects::VIEWERS {
            MsgKind::Views
        } else if subject == crate::subjects::DEBUG_REPORTS {
            MsgKind::Debug
        } else {
            MsgKind::Control
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Bytes published per kind by this process, reset at every stats report.
pub static OUT_BY_KIND: [Counter; 6] = [
    Counter::new(),
    Counter::new(),
    Counter::new(),
    Counter::new(),
    Counter::new(),
    Counter::new(),
];

/// Messages that couldn't be sent, per kind, and when we last warned about each kind.
static DROPPED: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static LAST_WARN_MS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// Warn about kind `kind` at most every 10 s, with how many were dropped since.
fn warn_dropped(kind: MsgKind, subject: &str, why: &str) {
    let i = kind.index();
    let dropped = DROPPED[i].fetch_add(1, Relaxed) + 1;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = LAST_WARN_MS[i].load(Relaxed);
    if now.saturating_sub(last) >= 10_000
        && LAST_WARN_MS[i]
            .compare_exchange(last, now, Relaxed, Relaxed)
            .is_ok()
    {
        DROPPED[i].store(0, Relaxed);
        warn!(kind = kind.name(), %subject, dropped, "{why} (warning at most every 10 s)");
    }
}

/// Encode in whichever format the bus is using right now.
pub fn encode_now<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    encode(value, binary())
}

pub fn encode<T: Serialize>(value: &T, binary: bool) -> Option<Vec<u8>> {
    let encoded = if binary {
        rmp_serde::to_vec(value).map_err(|e| e.to_string())
    } else {
        serde_json::to_vec(value).map_err(|e| e.to_string())
    };
    encoded.map_err(|err| warn!(%err, "encode failed")).ok()
}

/// Decode either format: JSON always starts with `{` or `[` (every message is a struct);
/// MessagePack never does for a struct (those bytes would mean a bare small integer).
pub fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, String> {
    match payload.first() {
        Some(b'{' | b'[') => serde_json::from_slice(payload).map_err(|e| e.to_string()),
        _ => rmp_serde::from_slice(payload).map_err(|e| e.to_string()),
    }
}

/// Subscribe to `subject` and receive decoded messages on a channel. Malformed messages are dropped.
pub async fn subscribe<T>(
    nats: &async_nats::Client,
    subject: &str,
) -> Result<mpsc::UnboundedReceiver<T>>
where
    T: DeserializeOwned + Send + 'static,
{
    subscribe_with(nats, subject, |value| value).await
}

/// Like `subscribe`, with when each message arrived (for the fake lag: a message is held back
/// from when it arrived, not from when it was next looked at).
pub async fn subscribe_timed<T>(
    nats: &async_nats::Client,
    subject: &str,
) -> Result<mpsc::UnboundedReceiver<(Instant, T)>>
where
    T: DeserializeOwned + Send + 'static,
{
    subscribe_with(nats, subject, |value| (Instant::now(), value)).await
}

async fn subscribe_with<T, U>(
    nats: &async_nats::Client,
    subject: &str,
    wrap: fn(T) -> U,
) -> Result<mpsc::UnboundedReceiver<U>>
where
    T: DeserializeOwned + Send + 'static,
    U: Send + 'static,
{
    let mut sub = nats.subscribe(subject.to_string()).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let subject = subject.to_string();
    tokio::spawn(async move {
        loop {
            // Stop (and unsubscribe) as soon as the reader is dropped.
            let msg = tokio::select! {
                msg = sub.next() => msg,
                _ = tx.closed() => break,
            };
            let Some(msg) = msg else { break };
            stats::BUS_IN.add(msg.payload.len());
            match decode::<T>(&msg.payload) {
                Ok(value) => {
                    if tx.send(wrap(value)).is_err() {
                        break;
                    }
                }
                Err(err) => warn!(%subject, %err, "dropping malformed bus message"),
            }
        }
    });
    Ok(rx)
}

pub async fn publish<T: Serialize>(nats: &async_nats::Client, subject: String, value: &T) {
    let Some(payload) = encode(value, binary()) else {
        return;
    };
    publish_bytes(nats, subject, payload).await;
}

/// Publish a message encoded earlier (see `encode_now`), e.g. one held back by the fake lag.
pub async fn publish_bytes(nats: &async_nats::Client, subject: String, payload: Vec<u8>) {
    let kind = MsgKind::of(&subject);
    let limit = nats.server_info().max_payload;
    if limit > 0 && payload.len() > limit {
        let why = format!(
            "message too big for the bus: {} KB, limit {} KB",
            payload.len() / 1024,
            limit / 1024
        );
        warn_dropped(kind, &subject, &why);
        return;
    }
    stats::BUS_OUT.add(payload.len());
    OUT_BY_KIND[kind.index()].add(payload.len());
    if let Err(err) = nats.publish(subject.clone(), payload.into()).await {
        warn_dropped(kind, &subject, &format!("publish failed: {err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;

    fn round_trip<T: Serialize + DeserializeOwned + std::fmt::Debug>(value: &T) -> (usize, usize) {
        let json = encode(value, false).unwrap();
        let bin = encode(value, true).unwrap();
        let from_json: T = decode(&json).unwrap();
        let from_bin: T = decode(&bin).unwrap();
        // Compare through JSON (not every type implements PartialEq).
        assert_eq!(
            serde_json::to_string(&from_json).unwrap(),
            serde_json::to_string(value).unwrap()
        );
        assert_eq!(
            serde_json::to_string(&from_bin).unwrap(),
            serde_json::to_string(value).unwrap()
        );
        (json.len(), bin.len())
    }

    #[test]
    fn every_kind_of_bus_message_survives_both_encodings() {
        let facts = Facts {
            id: 5,
            group: 2,
            x: 1.5,
            y: 2.5,
            tick: 99,
        };
        let bytes = vec![0x82, 0xa1, 0x61, 0x01, 0xa1, 0x62, 0xc3];
        let parts = vec![
            (0, bytes::Bytes::from(bytes.clone())),
            (2, bytes::Bytes::from(vec![0x05])),
        ];
        let envelopes = [
            Envelope {
                to: Address::Entity { id: 9 },
                from: facts,
                via: 2,
                seq: 1,
                trace: 4,
                hops: 0,
                kind: Kind::Input {
                    at_tick: 100,
                    sub: 2,
                    input: bytes.clone(),
                },
            },
            Envelope {
                to: Address::Area {
                    x: 3.0,
                    y: 4.0,
                    r: 2.5,
                },
                from: facts,
                via: 2,
                seq: 2,
                trace: 5,
                hops: 1,
                kind: Kind::Action {
                    msg: 3,
                    body: bytes.clone(),
                },
            },
            Envelope {
                to: Address::Group { id: 4 },
                from: facts,
                via: 3,
                seq: 4,
                trace: 7,
                hops: 0,
                kind: Kind::Handoff {
                    entity: bytes.clone(),
                },
            },
            Envelope {
                to: Address::Everyone,
                from: facts,
                via: 3,
                seq: 5,
                trace: 8,
                hops: 0,
                kind: Kind::Command {
                    msg: 1,
                    body: bytes.clone(),
                },
            },
        ];
        for env in &envelopes {
            round_trip(env);
        }
        let report = GroupReport {
            group: 3,
            tick: 1000,
            entities: (0..50)
                .map(|id| Reported {
                    id,
                    x: 12.25 + id as f32,
                    y: 30.5,
                    player: id % 2 == 0,
                    kind: 1,
                    parts: parts.clone(),
                })
                .collect(),
            queries: vec![SenseQuery {
                id: 1,
                x: 2.0,
                y: 3.0,
                r: 6.0,
                nearest: 8,
                target: Some(4),
                only: Some(vec![0, 2]),
            }],
            events: vec![EventAt {
                x: 1.0,
                y: 2.0,
                msg: 4,
                body: bytes.clone(),
                involves: vec![1],
            }],
        };
        let (json, bin) = round_trip(&report);
        assert!(bin * 2 < json, "binary {bin} vs json {json}");
        let seen = Seen {
            id: 1,
            x: 2.0,
            y: 3.0,
            group: 3,
            player: false,
            kind: 0,
            parts: parts.clone(),
        };
        round_trip(&SenseAnswers {
            spatial: 0x8001,
            tick: 1000,
            answers: vec![SenseAnswer {
                id: 2,
                seen: vec![seen; 8],
            }],
        });
        round_trip(&GroupCommand {
            to: Some(3),
            kind: GroupCommandKind::Leave { id: 4 },
        });
        round_trip(&GroupCommand {
            to: None,
            kind: GroupCommandKind::Release {
                moves: vec![(1, 2), (3, 4)],
            },
        });
        round_trip(&GroupCommand {
            to: None,
            kind: GroupCommandKind::Spawn {
                entities: vec![NewEntity {
                    id: 7,
                    x: 1.0,
                    y: 2.0,
                    player: true,
                    kind: 2,
                    parts: parts.clone(),
                }],
            },
        });
        round_trip(&SpatialViews {
            spatial: 0x8001,
            tick: 5,
            views: vec![CellView {
                player: 1,
                entities: vec![ViewEntity {
                    id: 1,
                    x: 1.0,
                    y: 2.0,
                    group: 3,
                    spatial: 0x8001,
                    player: true,
                    kind: 0,
                    parts: parts.clone(),
                }],
                events: vec![WireEvent {
                    msg: 4,
                    body: bytes.clone(),
                }],
                senses: vec![],
            }],
        });
        round_trip(&Assignment {
            version: 3,
            binary_bus: true,
            group_lag: vec![(3, 200)],
            ..Default::default()
        });
        round_trip(&Heartbeat {
            role: "group".into(),
            id: 3,
            entities: vec![(1, 2.0, 3.0)],
            groups: vec![3, 4],
        });
    }

    #[test]
    fn subjects_map_to_kinds() {
        use crate::subjects::*;
        assert_eq!(
            MsgKind::of(&spatial_reports_subject(0x8001)),
            MsgKind::Reports
        );
        assert_eq!(MsgKind::of(&group_senses_subject(3)), MsgKind::Senses);
        assert_eq!(MsgKind::of(&group_inbox_subject(3)), MsgKind::Envelopes);
        assert_eq!(MsgKind::of(BROADCAST), MsgKind::Envelopes);
        assert_eq!(MsgKind::of(&entity_subject(3)), MsgKind::Envelopes);
        assert_eq!(MsgKind::of(VIEWS), MsgKind::Views);
        assert_eq!(MsgKind::of(DEBUG_REPORTS), MsgKind::Debug);
        assert_eq!(MsgKind::of(HEARTBEATS), MsgKind::Control);
    }
}
