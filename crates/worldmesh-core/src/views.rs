//! Helpers shared by the gateway and the test world: merging a player's view from the parts
//! every Spatial Layer Cell sends, and addressing a player's input or command to their entity.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::protocol::{
    Address, EntityId, Envelope, Facts, Kind, SenseDebug, SpatialViews, ViewEntity, WireEvent,
};
use crate::{codec, time};

/// Numbers players' inputs and commands (unique per process, for ordering and dedupe).
static SEQ: AtomicU64 = AtomicU64::new(1);

/// One player's view: their parts of every Spatial Layer Cell's views, merged. An entity near
/// a border can be in two cells' parts (it moved between reports); keep the newest copy.
pub fn merge_view(
    player: EntityId,
    fresh: &[SpatialViews],
    new_events: &BTreeSet<u32>,
) -> (Vec<ViewEntity>, Vec<WireEvent>, Vec<SenseDebug>) {
    let mut entities: BTreeMap<EntityId, (u64, ViewEntity)> = BTreeMap::new();
    let mut events = Vec::new();
    let mut senses: BTreeMap<EntityId, SenseDebug> = BTreeMap::new();
    for cell in fresh {
        let Some(v) = cell.views.iter().find(|v| v.player == player) else {
            continue;
        };
        for e in &v.entities {
            if entities.get(&e.id).is_none_or(|(t, _)| cell.tick > *t) {
                entities.insert(e.id, (cell.tick, e.clone()));
            }
        }
        if new_events.contains(&cell.spatial) {
            events.extend(v.events.iter().cloned());
            for s in &v.senses {
                senses
                    .entry(s.id)
                    .and_modify(|m| {
                        m.found += s.found;
                        m.spatial.extend(s.spatial.iter().copied());
                    })
                    .or_insert_with(|| s.clone());
            }
        }
    }
    (
        entities.into_values().map(|(_, e)| e).collect(),
        events,
        senses.into_values().collect(),
    )
}

/// An envelope from a player (through a gateway) to their own entity.
pub fn from_player(id: EntityId, kind: Kind, traced: bool) -> Envelope {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let trace = if traced {
        codec::splitmix64(seq ^ (u64::from(id) << 40))
    } else {
        0
    };
    let from = Facts {
        id,
        group: 0,
        x: 0.0,
        y: 0.0,
        tick: time::current_tick(),
    };
    Envelope {
        to: Address::Entity { id },
        from,
        via: 0,
        seq,
        trace,
        hops: 0,
        kind,
        ack: None,
        resends: 0,
    }
}
