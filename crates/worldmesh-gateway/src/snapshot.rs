//! Optimized snapshots: binary, and only what changed since the last one sent to that player.
//!
//! The gateway remembers what it last sent each player about each entity; every tick it sends:
//!
//! - **entered**: entities newly in view, in full,
//! - **updated**: only entities with something new, and only those fields (an entity standing
//!   still costs nothing; a component is sent only when its bytes change, and a removed one by
//!   its id alone),
//! - **left**: entities out of view for `LEAVE_AFTER` ticks (so a one-tick gap mid-handoff
//!   doesn't make an entity leave and come back),
//! - the game's events (message id and MessagePack, as encoded by the group), and debug
//!   traffic as JSON, only if the player asked for it.
//!
//! Component, message and kind ids index the world's schema (sent in the welcome).
//!
//! Other entities' positions are `u16` steps of `1 / pos_scale` tiles (sent in the welcome);
//! the player's own position is exact (4-byte floats), because prediction compares it bit for bit.
//!
//! Layout, little endian (the client's `snapshot.ts` reads exactly this):
//!
//! ```text
//! u8 kind (1; 2 = first after a reset: forget everything before applying) · u64 tick
//! u16 entered, then per entity: u32 id · u8 flags (bit 0: player) · u16 kind · fields (all)
//! u16 updated, then per entity: u32 id · fields
//! u16 left, then per entity: u32 id
//! u16 events, then per event: u16 message · u32 length · MessagePack
//! u32 debug length · debug (JSON: {"senses":[…],"bus":[…],"streams":[…]}, empty parts left out)
//!
//! fields: u8 mask, then in this order whatever the mask says:
//!   POS     own entity: f32 x · f32 y; others: u16 x · u16 y (tiles × pos_scale)
//!   GROUP   u16
//!   SPATIAL u16
//!   PARTS   u8 count, then per component (new or changed): u16 component · u32 length · MessagePack
//!   GONE    u8 count, then per component (removed): u16 component
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use worldmesh_core::protocol::{BusMsg, EntityId, Part, SenseDebug, StreamMsg, ViewEntity, WireEvent};

pub const KIND_SNAPSHOT: u8 = 1;
/// The first snapshot after a reset: the client forgets what it had.
pub const KIND_FULL: u8 = 2;
pub const POS: u8 = 1;
pub const GROUP: u8 = 2;
pub const SPATIAL: u8 = 4;
pub const PARTS: u8 = 8;
pub const GONE: u8 = 16;
/// What an entity entering view is sent with (all of it: there's nothing to remove yet).
pub const ALL: u8 = POS | GROUP | SPATIAL | PARTS;
/// Ticks an entity must be missing from a player's view before they're told it left.
pub const LEAVE_AFTER: u8 = 5;

/// What a player was last sent about one entity.
#[derive(Debug, Clone, PartialEq)]
struct Sent {
    pos: Pos,
    group: u16,
    spatial: u16,
    /// Its visible components, sorted by id.
    parts: Vec<Part>,
    /// Ticks in a row it wasn't in the player's view.
    missing: u8,
}

#[derive(Serialize)]
struct Debug<'a> {
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    senses: &'a [SenseDebug],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    bus: &'a [BusMsg],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    streams: &'a [StreamMsg],
}

/// One player's encoder: remembers what that player has been sent.
pub struct Encoder {
    pos_scale: f32,
    known: BTreeMap<EntityId, Sent>,
    /// Whether a snapshot went out since the last reset.
    started: bool,
}

impl Encoder {
    pub fn new(pos_scale: f32) -> Self {
        Self { pos_scale, known: BTreeMap::new(), started: false }
    }

    /// Forget everything (the next snapshot re-sends every entity in full).
    pub fn reset(&mut self) {
        self.known.clear();
        self.started = false;
    }

    /// The snapshot for `player` at `tick`. Debug parts are empty unless they asked for them.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        player: EntityId,
        tick: u64,
        entities: &[ViewEntity],
        events: &[WireEvent],
        senses: &[SenseDebug],
        bus: &[BusMsg],
        streams: &[StreamMsg],
    ) -> Vec<u8> {
        let (mut entered, mut updated, mut left) = (Vec::new(), Vec::new(), Vec::new());
        let (mut n_entered, mut n_updated) = (0u16, 0u16);
        for e in entities {
            let now = self.sent_state(player, e);
            match self.known.get_mut(&e.id) {
                None => {
                    put_u32(&mut entered, e.id);
                    entered.push(u8::from(e.player));
                    entered.extend_from_slice(&e.kind.to_le_bytes());
                    put_fields(&mut entered, &now, &now.parts, &[]);
                    self.known.insert(e.id, now);
                    n_entered += 1;
                }
                Some(before) => {
                    let (new, gone) = diff(&before.parts, &now.parts);
                    let mut mask = 0;
                    if before.pos != now.pos {
                        mask |= POS;
                    }
                    if before.group != now.group {
                        mask |= GROUP;
                    }
                    if before.spatial != now.spatial {
                        mask |= SPATIAL;
                    }
                    if !new.is_empty() {
                        mask |= PARTS;
                    }
                    if !gone.is_empty() {
                        mask |= GONE;
                    }
                    if mask != 0 {
                        put_u32(&mut updated, e.id);
                        updated.push(mask);
                        put_masked(&mut updated, mask, &now, &new, &gone);
                        n_updated += 1;
                    }
                    *before = now;
                }
            }
        }
        // Whoever wasn't in the view: gone after a few ticks.
        let present: BTreeSet<EntityId> = entities.iter().map(|e| e.id).collect();
        let mut n_left = 0u16;
        self.known.retain(|&id, s| {
            if present.contains(&id) {
                return true;
            }
            s.missing += 1;
            if s.missing < LEAVE_AFTER {
                return true;
            }
            put_u32(&mut left, id);
            n_left += 1;
            false
        });

        let mut out = Vec::with_capacity(24 + entered.len() + updated.len() + left.len());
        out.push(if self.started { KIND_SNAPSHOT } else { KIND_FULL });
        self.started = true;
        out.extend_from_slice(&tick.to_le_bytes());
        for (n, part) in [(n_entered, &entered), (n_updated, &updated), (n_left, &left)] {
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(part);
        }
        out.extend_from_slice(&(events.len().min(u16::MAX as usize) as u16).to_le_bytes());
        for ev in events.iter().take(u16::MAX as usize) {
            out.extend_from_slice(&ev.msg.to_le_bytes());
            put_u32(&mut out, ev.body.len() as u32);
            out.extend_from_slice(&ev.body);
        }
        let empty = senses.is_empty() && bus.is_empty() && streams.is_empty();
        let debug = if empty { Vec::new() } else { serde_json::to_vec(&Debug { senses, bus, streams }).unwrap_or_default() };
        put_u32(&mut out, debug.len() as u32);
        out.extend_from_slice(&debug);
        out
    }

    fn sent_state(&self, player: EntityId, e: &ViewEntity) -> Sent {
        let pos = if e.id == player {
            Pos::Exact(e.x, e.y)
        } else {
            Pos::Steps(quantize(e.x, self.pos_scale), quantize(e.y, self.pos_scale))
        };
        let mut parts = e.parts.clone();
        parts.sort_by_key(|(c, _)| *c);
        Sent { pos, group: e.group as u16, spatial: e.spatial as u16, parts, missing: 0 }
    }
}

/// A position as sent: exact for the player's own entity, in steps for others.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pos {
    Exact(f32, f32),
    Steps(u16, u16),
}

/// What changed between two sorted component lists: (new or changed, removed).
fn diff<'a>(before: &[Part], now: &'a [Part]) -> (Vec<&'a Part>, Vec<u16>) {
    let new = now.iter().filter(|p| !before.contains(p)).collect();
    let gone = before.iter().map(|(c, _)| *c).filter(|c| !now.iter().any(|(n, _)| n == c)).collect();
    (new, gone)
}

/// Every field, for an entity entering view.
fn put_fields(out: &mut Vec<u8>, s: &Sent, parts: &[Part], gone: &[u16]) {
    let refs: Vec<&Part> = parts.iter().collect();
    out.push(ALL);
    put_masked(out, ALL, s, &refs, gone);
}

/// The fields `mask` names, in the order the format gives.
fn put_masked(out: &mut Vec<u8>, mask: u8, s: &Sent, parts: &[&Part], gone: &[u16]) {
    if mask & POS != 0 {
        match s.pos {
            Pos::Exact(x, y) => {
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
            }
            Pos::Steps(x, y) => {
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
            }
        }
    }
    if mask & GROUP != 0 {
        out.extend_from_slice(&s.group.to_le_bytes());
    }
    if mask & SPATIAL != 0 {
        out.extend_from_slice(&s.spatial.to_le_bytes());
    }
    if mask & PARTS != 0 {
        out.push(parts.len().min(255) as u8);
        for (c, bytes) in parts.iter().take(255) {
            out.extend_from_slice(&c.to_le_bytes());
            put_u32(out, bytes.len() as u32);
            out.extend_from_slice(bytes);
        }
    }
    if mask & GONE != 0 {
        out.push(gone.len().min(255) as u8);
        for c in gone.iter().take(255) {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
}

fn quantize(v: f32, scale: f32) -> u16 {
    (v * scale).round().clamp(0.0, u16::MAX as f32) as u16
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_bytes::ByteBuf;

    const SCALE: f32 = 1023.0;

    /// An entity as a client has it.
    #[derive(Debug, Clone, PartialEq)]
    struct Known {
        x: f32,
        y: f32,
        group: u32,
        spatial: u32,
        player: bool,
        kind: u16,
        parts: BTreeMap<u16, Vec<u8>>,
    }

    /// A reader for the format, as the client does it, to check what the encoder wrote.
    #[derive(Default)]
    struct Decoder {
        known: BTreeMap<u32, Known>,
    }

    struct Decoded {
        tick: u64,
        counts: (usize, usize, usize),
        events: Vec<(u16, Vec<u8>)>,
        debug: usize,
    }

    impl Decoder {
        fn decode(&mut self, me: u32, buf: &[u8]) -> Decoded {
            let mut at = 0;
            let take = |at: &mut usize, n: usize| {
                *at += n;
                &buf[*at - n..*at]
            };
            let u8_ = |at: &mut usize| take(at, 1)[0];
            let u16_ = |at: &mut usize| u16::from_le_bytes(take(at, 2).try_into().unwrap());
            let u32_ = |at: &mut usize| u32::from_le_bytes(take(at, 4).try_into().unwrap());
            let kind = u8_(&mut at);
            assert!(kind == KIND_SNAPSHOT || kind == KIND_FULL);
            if kind == KIND_FULL {
                self.known.clear();
            }
            let tick = u64::from_le_bytes(take(&mut at, 8).try_into().unwrap());
            let fields = |at: &mut usize, id: u32, e: &mut Known| {
                let mask = u8_(at);
                if mask & POS != 0 {
                    if id == me {
                        e.x = f32::from_bits(u32_(at));
                        e.y = f32::from_bits(u32_(at));
                    } else {
                        e.x = f32::from(u16_(at)) / SCALE;
                        e.y = f32::from(u16_(at)) / SCALE;
                    }
                }
                if mask & GROUP != 0 {
                    e.group = u32::from(u16_(at));
                }
                if mask & SPATIAL != 0 {
                    e.spatial = u32::from(u16_(at));
                }
                if mask & PARTS != 0 {
                    for _ in 0..u8_(at) {
                        let c = u16_(at);
                        let n = u32_(at) as usize;
                        e.parts.insert(c, take(at, n).to_vec());
                    }
                }
                if mask & GONE != 0 {
                    for _ in 0..u8_(at) {
                        e.parts.remove(&u16_(at));
                    }
                }
            };
            let n = u16_(&mut at) as usize;
            for _ in 0..n {
                let id = u32_(&mut at);
                let player = u8_(&mut at) & 1 != 0;
                let kind = u16_(&mut at);
                let mut e = Known { x: 0.0, y: 0.0, group: 0, spatial: 0, player, kind, parts: BTreeMap::new() };
                fields(&mut at, id, &mut e);
                self.known.insert(id, e);
            }
            let u = u16_(&mut at) as usize;
            for _ in 0..u {
                let id = u32_(&mut at);
                let mut e = self.known.remove(&id).expect("updated entity was known");
                fields(&mut at, id, &mut e);
                self.known.insert(id, e);
            }
            let l = u16_(&mut at) as usize;
            for _ in 0..l {
                let id = u32_(&mut at);
                self.known.remove(&id);
            }
            let ne = u16_(&mut at) as usize;
            let events = (0..ne)
                .map(|_| {
                    let msg = u16_(&mut at);
                    let n = u32_(&mut at) as usize;
                    (msg, take(&mut at, n).to_vec())
                })
                .collect();
            let debug = u32_(&mut at) as usize;
            at += debug;
            assert_eq!(at, buf.len(), "read exactly what was written");
            Decoded { tick, counts: (n, u, l), events, debug }
        }
    }

    fn part(c: u16, bytes: &[u8]) -> Part {
        (c, ByteBuf::from(bytes.to_vec()))
    }

    /// Two components: 0 is health (100), 1 is a name.
    fn entity(id: u32, x: f32, y: f32) -> ViewEntity {
        let parts = vec![part(0, &[100]), part(1, &[0xa1, b'a'])];
        ViewEntity { id, x, y, group: 7, spatial: 0x8001, player: id == 1, kind: 3, parts }
    }

    #[test]
    fn round_trips_and_sends_only_what_changed() {
        let (mut enc, mut dec) = (Encoder::new(SCALE), Decoder::default());
        let mut world = vec![entity(1, 10.123_457, 20.5), entity(2, 30.0, 40.0), entity(3, 5.0, 5.0)];
        let first = enc.encode(1, 1000, &world, &[], &[], &[], &[]);
        assert_eq!(first[0], KIND_FULL);
        let d = dec.decode(1, &first);
        assert_eq!((d.tick, d.counts), (1000, (3, 0, 0)));
        assert_eq!(dec.known[&1].x, 10.123_457, "our own position is exact");
        assert!(dec.known[&1].player && !dec.known[&2].player);
        assert_eq!((dec.known[&2].kind, dec.known[&2].parts.len()), (3, 2));
        assert!((dec.known[&2].x - 30.0).abs() < 1.0 / SCALE);

        // Entity 2 moves; entity 3's health changes and it loses its name.
        world[1].x = 30.25;
        world[2].parts = vec![part(0, &[80])];
        let second = enc.encode(1, 1001, &world, &[], &[], &[], &[]);
        let d = dec.decode(1, &second);
        assert_eq!(d.counts, (0, 2, 0));
        assert!((dec.known[&2].x - 30.25).abs() < 1.0 / SCALE);
        assert_eq!(dec.known[&3].parts, BTreeMap::from([(0, vec![80])]));
        assert!(second.len() < 48, "{} bytes for two small changes", second.len());

        // Nothing changes: nearly empty.
        let third = enc.encode(1, 1002, &world, &[], &[], &[], &[]);
        assert_eq!(third.len(), 1 + 8 + 2 + 2 + 2 + 2 + 4);
    }

    #[test]
    fn only_the_component_that_changed_is_sent() {
        let (mut enc, mut dec) = (Encoder::new(SCALE), Decoder::default());
        let big = vec![0xc4; 200];
        let mut e = entity(2, 1.0, 1.0);
        e.parts.push(part(5, &big));
        dec.decode(1, &enc.encode(1, 1, std::slice::from_ref(&e), &[], &[], &[], &[]));
        e.parts[0] = part(0, &[99]);
        let second = enc.encode(1, 2, std::slice::from_ref(&e), &[], &[], &[], &[]);
        dec.decode(1, &second);
        assert!(second.len() < 40, "the big component isn't re-sent: {} bytes", second.len());
        assert_eq!(dec.known[&2].parts[&5], big);
    }

    #[test]
    fn events_and_debug_ride_along() {
        let (mut enc, mut dec) = (Encoder::new(SCALE), Decoder::default());
        let ev = WireEvent { msg: 4, body: vec![0x81, 0xa1, b'e', 0xa3, b'h', b'i', b't'] };
        let sense = SenseDebug { id: 1, x: 1.0, y: 1.0, r: 6.0, found: 2, spatial: vec![0x8001] };
        let d = dec.decode(1, &enc.encode(1, 5, &[entity(1, 1.0, 1.0)], &[ev.clone(), ev.clone()], &[sense], &[], &[]));
        assert_eq!(d.events, vec![(4, ev.body.clone()), (4, ev.body)]);
        assert!(d.debug > 0);
    }

    #[test]
    fn an_entity_leaves_only_after_missing_a_few_ticks() {
        let (mut enc, mut dec) = (Encoder::new(SCALE), Decoder::default());
        let world = vec![entity(1, 1.0, 1.0), entity(2, 2.0, 2.0)];
        dec.decode(1, &enc.encode(1, 1, &world, &[], &[], &[], &[]));
        for t in 2..2 + u64::from(LEAVE_AFTER) - 1 {
            let d = dec.decode(1, &enc.encode(1, t, &world[..1], &[], &[], &[], &[]));
            assert_eq!((dec.known.len(), d.counts.2), (2, 0), "a short gap (mid-handoff) isn't a leave");
        }
        let d = dec.decode(1, &enc.encode(1, 99, &world[..1], &[], &[], &[], &[]));
        assert_eq!((dec.known.len(), d.counts.2), (1, 1));
    }

    #[test]
    fn far_smaller_than_json_for_a_moving_crowd() {
        let mut enc = Encoder::new(SCALE);
        let mut world: Vec<ViewEntity> = (0..1600).map(|i| entity(i + 2, (i % 64) as f32 + 0.5, (i / 64) as f32 + 0.5)).collect();
        enc.encode(1, 1, &world, &[], &[], &[], &[]);
        for e in &mut world {
            e.x += 0.5; // everyone moves
        }
        let compact = enc.encode(1, 2, &world, &[], &[], &[], &[]).len();
        let json = serde_json::to_vec(&world).unwrap().len();
        assert!(compact * 10 < json, "compact {compact} bytes vs JSON {json}");
    }

    /// The client's decoder (`packages/client`) is checked against snapshots this encoder
    /// wrote, kept in a fixture file. Run with `WORLDMESH_WRITE_FIXTURES=1` to rewrite it after
    /// changing the format (then run the client's tests).
    #[test]
    fn the_client_fixture_matches_what_the_encoder_writes() {
        use serde_json::json;
        use worldmesh_core::codec::{bytes_to_json, to_bytes};
        // The world's schema, as the client gets it in the welcome.
        let schema = json!({ "components": ["health", "name", "burning"], "messages": ["hit"], "kinds": ["hero", "bot"] });
        let names = ["health", "name", "burning"];
        let me = 7;
        let entity = |id: u32, x: f32, y: f32, group: u32, hp: u32, burning: bool| {
            let mut parts = vec![(0, ByteBuf::from(to_bytes(&hp).unwrap())), (1, ByteBuf::from(to_bytes(&format!("e{id}")).unwrap()))];
            if burning {
                parts.push((2, ByteBuf::from(to_bytes(&json!({ "ticks": 3 })).unwrap())));
            }
            ViewEntity { id, x, y, group, spatial: 0x8001, player: id < 100, kind: u16::from(id >= 100), parts }
        };
        let event = WireEvent { msg: 0, body: to_bytes(&json!({ "target": 101, "damage": 12 })).unwrap() };
        let sense = SenseDebug { id: 101, x: 3.0, y: 4.0, r: 6.0, found: 2, spatial: vec![0x8001] };
        let mut enc = Encoder::new(SCALE);
        let first = vec![
            entity(me, 10.123_4, 20.5, 1, 100, false),
            entity(101, 12.25, 20.75, 2, 100, true),
            entity(102, 30.0, 31.0, 2, 50, false),
        ];
        let a = enc.encode(me, 1000, &first, &[], &[], &[], &[]);
        // 7 moved; 101 got hurt, stopped burning and moved group; 102 is gone (5 ticks later it leaves).
        let second = vec![entity(me, 10.623_4, 20.5, 1, 100, false), entity(101, 12.25, 20.75, 3, 88, false)];
        let rest: Vec<Vec<u8>> = (1001..=1005)
            .map(|tick| enc.encode(me, tick, &second, std::slice::from_ref(&event), std::slice::from_ref(&sense), &[], &[]))
            .collect();
        let q = |v: f32| f64::from(quantize(v, SCALE)) / f64::from(SCALE);
        let shown = |e: &ViewEntity| {
            let (x, y) = if e.id == me { (f64::from(e.x), f64::from(e.y)) } else { (q(e.x), q(e.y)) };
            let view: serde_json::Map<String, serde_json::Value> =
                e.parts.iter().map(|(c, b)| (names[*c as usize].to_string(), bytes_to_json(b))).collect();
            let kind = ["hero", "bot"][e.kind as usize];
            json!({ "id": e.id, "x": x, "y": y, "group": e.group, "spatial": e.spatial, "player": e.player,
                    "kind": kind, "view": view })
        };
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let fixture = json!({
            "me": me,
            "pos_scale": SCALE,
            "schema": schema,
            // Applied in order; what the client should have is given after the first and the last.
            "snapshots": std::iter::once(json!({ "hex": hex(&a), "tick": 1000,
                    "entities": first.iter().map(shown).collect::<Vec<_>>(), "events": [], "senses": [] }))
                .chain(rest.iter().enumerate().map(|(i, b)| {
                    let tick = 1001 + i as u64;
                    if tick < 1005 {
                        json!({ "hex": hex(b), "tick": tick })
                    } else {
                        json!({ "hex": hex(b), "tick": tick, "entities": second.iter().map(shown).collect::<Vec<_>>(),
                                "events": [{ "name": "hit", "data": bytes_to_json(&event.body) }], "senses": [sense] })
                    }
                }))
                .collect::<Vec<_>>(),
        });
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/client/test/fixtures/snapshots.json");
        let text = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
        if std::env::var_os("WORLDMESH_WRITE_FIXTURES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
        }
        let saved = std::fs::read_to_string(&path).expect("fixture missing: run with WORLDMESH_WRITE_FIXTURES=1");
        assert_eq!(saved, text, "the snapshot format changed: rewrite the fixture and check the client");
    }
}
