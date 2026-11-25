//! Bus subjects (NATS topics) and id conventions. Everything Worldmesh sends on the bus goes to
//! one of these.

use crate::protocol::{EntityId, GroupId};

/// Group, group process or Spatial Layer Cell -> orchestrator: "I'm alive (and here's what I hold)".
pub const HEARTBEATS: &str = "control.heartbeat";
/// Orchestrator -> everyone: which groups and cells are in use, and who owns which chunk.
pub const ASSIGNMENTS: &str = "control.assignments";
/// Gateway -> orchestrator: debug panel commands.
pub const DEBUG_COMMANDS: &str = "control.debug";
/// Gateway -> orchestrator: a player connected (the orchestrator picks their group).
pub const JOINS: &str = "control.joins";
/// Gateways -> orchestrator, about once a second: player ids wanted, and the highest one each
/// has been given. The orchestrator answers on `gateway_ids_subject`.
pub const PLAYER_IDS: &str = "control.player_ids";
/// Groups -> orchestrator: the game spawned entities (the orchestrator gives ids, picks groups).
pub const SPAWNS: &str = "control.spawns";
/// Orchestrator or gateway -> groups: spawn, release, leave.
pub const GROUP_COMMANDS: &str = "control.groups";
/// Orchestrator -> group processes: start or stop groups (a process can run any number).
pub const HOST_COMMANDS: &str = "control.hosts";
/// Every server process -> gateways: CPU, memory and traffic, once a second.
pub const STATS: &str = "control.stats";
/// Actions for every entity in every group.
pub const BROADCAST: &str = "group.all.inbox";
/// Each gateway -> every Spatial Layer Cell, every tick: where its players are looking.
pub const VIEWERS: &str = "spatial.viewers";
/// Every server process -> gateway, every tick: what it sent and received (debug views only).
pub const DEBUG_REPORTS: &str = "debug.reports";

/// Envelopes for one entity. Only the group simulating it subscribes (it subscribes when it
/// takes the entity and keeps listening a little after handing it off, forwarding strays).
pub fn entity_subject(entity: EntityId) -> String {
    format!("entity.{entity}")
}

/// Envelopes for one group: handoffs, forwards, area actions passed on by Spatial Layer Cells.
/// One subject, so messages from any one sender arrive in the order they were sent: an action
/// forwarded after a handoff never overtakes the handoff.
pub fn group_inbox_subject(group: GroupId) -> String {
    format!("group.{group}.inbox")
}

/// Confirmations for one group: the sure actions and handoffs it sent arrived.
pub fn group_acks_subject(group: GroupId) -> String {
    format!("group.{group}.acks")
}

/// Confirmations for one Spatial Layer Cell: the area actions it passed on arrived.
pub fn spatial_acks_subject(spatial: GroupId) -> String {
    format!("spatial.{spatial}.acks")
}

/// Confirmations for one gateway: its players' commands arrived.
pub fn gateway_acks_subject(gateway: u32) -> String {
    format!("gateway.{gateway}.acks")
}

/// Spatial Layer Cells -> one group: results of its entities' spatial queries.
pub fn group_senses_subject(group: GroupId) -> String {
    format!("group.{group}.senses")
}

/// Groups -> one Spatial Layer Cell, every tick: their entities in its area, their questions
/// about it, and what happened in it.
pub fn spatial_reports_subject(spatial: GroupId) -> String {
    format!("spatial.{spatial}.reports")
}

/// Area actions for one Spatial Layer Cell to pass on to whoever is in the area.
pub fn spatial_inbox_subject(spatial: GroupId) -> String {
    format!("spatial.{spatial}.inbox")
}

/// Spatial Layer Cells -> one gateway, every tick: what each of its players can see in that
/// cell's area. Each gateway gets only its own players' views.
pub fn gateway_views_subject(gateway: u32) -> String {
    format!("gateway.{gateway}.views")
}

/// Orchestrator -> one gateway: player ids for it to give out.
pub fn gateway_ids_subject(gateway: u32) -> String {
    format!("gateway.{gateway}.ids")
}

/// Spatial Layer Cell ids have this bit set and group ids don't, so a bare id says which it is.
pub const SPATIAL_ID_BIT: u32 = 0x8000;

pub fn is_spatial_id(id: u32) -> bool {
    id & SPATIAL_ID_BIT != 0
}

/// A fresh random id for a gateway: any u32 but 0. Gateways can run by the thousand, so they
/// draw from all 32 bits.
pub fn random_gateway_id() -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    (h.finish() as u32).max(1)
}

/// A fresh random id for a group (or, with `spatial`, a Spatial Layer Cell).
pub fn random_process_id(spatial: bool) -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    let raw = (h.finish() as u32 % 0x7ffe) + 1; // 1..=0x7ffe, never 0
    if spatial {
        raw | SPATIAL_ID_BIT
    } else {
        raw & !SPATIAL_ID_BIT
    }
}
