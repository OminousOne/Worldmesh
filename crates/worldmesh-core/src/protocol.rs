//! Every message Worldmesh sends: between the browser and the gateway, and between server
//! processes on the bus.
//!
//! The game's own data (components, inputs, commands, actions, events) travels as MessagePack
//! bytes tagged with ids from the world's [`Schema`]. Only the group hosts, which run the
//! game's code, decode it. Spatial Layer Cells and the orchestrator never need to know what a
//! game is; the gateway only checks what players send and names things for JSON clients.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::WorldConfig;

/// An entity's id, unique across the world. Both kinds come from the orchestrator: players'
/// through the gateway they connect to (which asks for a few at a time), and entities the game
/// spawns directly (see [`SPAWNED_ID_START`]).
pub type EntityId = u32;
/// A Server Entity Group's id (or a Spatial Layer Cell's, see [`crate::subjects::is_spatial_id`]).
pub type GroupId = u32;

/// One component of an entity, encoded: (component id, MessagePack). Ids index the world's
/// [`Schema::components`]. The bytes are shared, not copied, when a cell puts the same
/// entity in many players' views.
pub type Part = (u16, bytes::Bytes);

/// The names behind the ids used on the wire, in id order. Sent to every player on connect.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    pub components: Vec<String>,
    pub messages: Vec<String>,
    pub kinds: Vec<String>,
}

/// A game event, encoded: which message it is, and its body (MessagePack).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WireEvent {
    pub msg: u16,
    #[serde(with = "serde_bytes")]
    pub body: Vec<u8>,
}

/// Ids from here up are for entities the game spawns; below are players'.
pub const SPAWNED_ID_START: EntityId = 1 << 31;

// ---- Browser <-> gateway ----

/// Browser -> gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// The game's input, to take effect at sub-step `sub` of tick `tick` (0: as soon as
    /// possible). The player's group applies it exactly then, so the client can predict it.
    Input {
        tick: u64,
        #[serde(default)]
        sub: u8,
        input: Value,
    },
    /// One of the game's commands, by name (a discrete request, like "use this ability"),
    /// handled by the player's own entity on the next tick.
    Command { name: String, command: Value },
    /// How this player wants snapshots: `compact` = binary, only what changed; `debug` =
    /// include debug traffic (for the debug panel's bus view and overlays).
    Settings {
        #[serde(default)]
        compact: bool,
        #[serde(default)]
        debug: bool,
        /// Every process's numbers once a second (the debug panel asks for them).
        #[serde(default)]
        stats: bool,
    },
    /// Debug panel controls (development only: anyone connected can send these).
    Debug {
        #[serde(default)]
        set_groups: Option<u32>,
        #[serde(default)]
        set_spatial: Option<u32>,
        #[serde(default)]
        regroup: Option<bool>,
        #[serde(default)]
        binary_bus: Option<bool>,
        /// Fake network lag for one Server Entity Group: [group, milliseconds] (0 for none).
        #[serde(default)]
        set_lag: Option<(GroupId, u32)>,
        /// Fake loss for the whole world: the % of envelopes and confirmations every process
        /// drops as they arrive (0 for none).
        #[serde(default)]
        set_loss: Option<u8>,
    },
}

/// Gateway -> browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        /// The player's entity id.
        id: EntityId,
        tick_ms: u64,
        world: WorldConfig,
        /// Other entities' positions in optimized snapshots are `u16` steps of `1 / pos_scale`.
        pos_scale: f32,
        /// What the game sends every player on connect (`World::welcome`), by key.
        game: Value,
        /// The names of the world's components, messages and kinds, in id order.
        schema: Schema,
    },
    /// What this player can see this tick, plus debug traffic if they asked for it.
    Snapshot {
        tick: u64,
        entities: Vec<SnapshotEntity>,
        /// The game's events on this player's screen this tick: `{"name": ..., "data": ...}`.
        #[serde(default)]
        events: Vec<Value>,
        #[serde(default)]
        senses: Vec<SenseDebug>,
        #[serde(default)]
        bus: Vec<BusMsg>,
        #[serde(default)]
        streams: Vec<StreamMsg>,
    },
    /// Which groups and Spatial Layer Cells are in use, and who owns which chunk.
    Ownership(Assignment),
    /// Sent only to the player who sent an input: how early it arrived, so the client can
    /// pick how far ahead of the server to run.
    InputAck {
        tick: u64,
        sub: u8,
        /// How long before the moment it describes the input reached the gateway (negative: late).
        early_ms: i64,
    },
    /// Once a second: every server process's numbers, for the debug panel's graphs.
    Stats { processes: Vec<ProcessStats> },
}

/// One entity in a JSON snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotEntity {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    /// The group simulating it and the Spatial Layer Cell whose area it stands in.
    pub group: GroupId,
    pub spatial: GroupId,
    /// Controlled by a connected player.
    pub player: bool,
    /// Its kind's name.
    pub kind: String,
    /// Its visible components, by name.
    pub view: Value,
}

// ---- Debug ----

/// A spatial query, as drawn by the debug overlay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SenseDebug {
    /// The entity that asked.
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    /// Entities found, over every Spatial Layer Cell that answered.
    pub found: u32,
    /// The Spatial Layer Cells that answered (more than one when the circle crosses areas).
    pub spatial: Vec<GroupId>,
}

/// One envelope that crossed the bus, reported by the process that received it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BusMsg {
    /// The process that put it on the bus (a group or Spatial Layer Cell id, 0 = gateway).
    pub from: u32,
    pub to: u32,
    pub kind: BusKind,
    pub trace: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BusKind {
    Handoff,
    /// A player's command, from the gateway to the player's group.
    Command,
    /// A player's input, from the gateway to the player's group.
    Input,
    /// An action aimed at one entity.
    Action,
    /// An action aimed at an area: group -> Spatial Layer Cell -> groups with entities there.
    Area,
    /// An action for every entity everywhere.
    Broadcast,
    /// Passed on by an old owner after a handoff.
    Forward,
    /// Sent again: it was sent for sure delivery and its sender heard no confirmation in time.
    Resend,
}

/// An every-tick stream between two processes, and how many items it carried this tick.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamMsg {
    pub from: u32,
    pub to: u32,
    pub kind: StreamKind,
    pub n: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Group -> Spatial Layer Cell: its entities in that area (n), plus questions and events.
    Report,
    /// Spatial Layer Cell -> group: spatial query results (n).
    Senses,
    /// Spatial Layer Cell -> gateway: players' views (n).
    View,
    /// Gateway -> Spatial Layer Cell: where players are looking (n).
    Viewers,
}

// ---- Control ----

/// Orchestrator -> everyone.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Assignment {
    pub version: u64,
    pub chunk_size: u32,
    pub chunks_x: u32,
    pub chunks_y: u32,
    /// Spatial Layer Cell owning each chunk, row-major (0 = nobody yet).
    pub owners: Vec<GroupId>,
    /// Spatial Layer Cells in use, in the order they own runs of the Hilbert curve.
    pub spatial: Vec<GroupId>,
    /// Server Entity Groups in use.
    pub groups: Vec<GroupId>,
    /// Running, in use or idle: Spatial Layer Cells, groups, and the processes running groups.
    pub spatial_running: u32,
    pub groups_running: u32,
    #[serde(default)]
    pub group_hosts: u32,
    /// Encode bus messages as binary (MessagePack) instead of JSON (debug toggle).
    #[serde(default)]
    pub binary_bus: bool,
    /// Fake network lag per Server Entity Group (debug): (group, milliseconds).
    #[serde(default)]
    pub group_lag: Vec<(GroupId, u32)>,
    /// Fake loss (debug): the % of envelopes and confirmations every process drops as they
    /// arrive.
    #[serde(default)]
    pub fake_loss: u8,
}

/// Every server process -> gateways (-> players), once a second.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessStats {
    /// "group" (one Server Entity Group), "group-host" (a process running groups), "spatial",
    /// "gateway", "orchestrator" or "nats".
    pub role: String,
    /// Group, group process, Spatial Layer Cell or gateway id; 0 for the other roles.
    pub id: u32,
    /// CPU use in % of one core (None where it can't be measured).
    pub cpu_pct: Option<f32>,
    pub mem_mb: Option<f32>,
    /// Bus messages received and published, per second.
    pub bus_in: Rate,
    pub bus_out: Rate,
    /// WebSocket messages from and to players, per second (gateway only).
    pub ws_in: Rate,
    pub ws_out: Rate,
    /// Players connected (gateway), entities simulated (group) or held (Spatial Layer Cell).
    pub players: u32,
    /// Time spent per tick, average and worst, in ms.
    pub tick_ms_avg: Option<f32>,
    pub tick_ms_max: Option<f32>,
    /// The share of each tick spent working, in % (100: no time left).
    #[serde(default)]
    pub load_pct: Option<f32>,
    /// Actions per second: delivered within the group, sent to other groups, received from
    /// them, and forwarded after a handoff.
    #[serde(default)]
    pub actions_local: f32,
    #[serde(default)]
    pub actions_sent: f32,
    #[serde(default)]
    pub actions_received: f32,
    #[serde(default)]
    pub actions_forwarded: f32,
    /// Entities handed off per second.
    #[serde(default)]
    pub handoffs: f32,
    /// Sure deliveries (actions, commands, inputs, handoffs) sent again per second because no
    /// confirmation came back in time, and given up on (never confirmed) per second.
    #[serde(default)]
    pub resent: f32,
    #[serde(default)]
    pub lost: f32,
    /// Spatial queries per second (group: made; Spatial Layer Cell: answered).
    #[serde(default)]
    pub queries: f32,
    /// Spatial Layer Cell: player views built, and area actions passed on, per second.
    #[serde(default)]
    pub views: f32,
    #[serde(default)]
    pub areas: f32,
    /// Spatial Layer Cell: players whose screen covers part of its area.
    #[serde(default)]
    pub viewers: u32,
    /// Bytes published per second by kind of message.
    #[serde(default)]
    pub bus_kinds: Vec<(String, f32)>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Rate {
    pub msgs: f32,
    pub bytes: f32,
}

/// Group, group process or Spatial Layer Cell -> orchestrator, twice a second.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Heartbeat {
    /// "group", "host" (a process running groups) or "spatial".
    pub role: String,
    pub id: u32,
    /// Groups: (entity, x, y) for every entity they simulate.
    #[serde(default)]
    pub entities: Vec<(EntityId, f32, f32)>,
    /// Group processes: the groups they run.
    #[serde(default)]
    pub groups: Vec<GroupId>,
}

/// Orchestrator -> one group process: start these groups, stop these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostCommand {
    pub host: u32,
    #[serde(default)]
    pub start: Vec<GroupId>,
    #[serde(default)]
    pub stop: Vec<GroupId>,
}

/// Gateway -> orchestrator: debug panel commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugCommand {
    #[serde(default)]
    pub set_groups: Option<u32>,
    #[serde(default)]
    pub set_spatial: Option<u32>,
    #[serde(default)]
    pub regroup: bool,
    #[serde(default)]
    pub binary_bus: Option<bool>,
    #[serde(default)]
    pub set_lag: Option<(GroupId, u32)>,
    #[serde(default)]
    pub set_loss: Option<u8>,
}

/// Gateway -> orchestrator: a player connected. Where they appear and as what come from the
/// world's join function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    pub id: EntityId,
    /// The gateway the player is connected to. If it stops, the orchestrator removes its
    /// players' entities.
    #[serde(default)]
    pub gateway: u32,
    pub x: f32,
    pub y: f32,
    pub kind: u16,
    /// Components on top of the kind's.
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// Gateway -> orchestrator, about once a second: how many player ids it wants (0: none just
/// now), and the highest id it has ever been given. A restarted orchestrator learns from these
/// where to carry on from, so it never hands out an id a gateway already has.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerIdRequest {
    pub gateway: u32,
    pub count: u32,
    pub above: EntityId,
}

/// Orchestrator -> one gateway: player ids `first .. first + count`, for it alone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerIds {
    pub first: EntityId,
    pub count: u32,
}

/// Group -> orchestrator: the game spawned entities. The orchestrator gives them ids and
/// picks their groups by where they stand.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub from_group: GroupId,
    pub entities: Vec<NewEntity>,
}

/// Orchestrator or gateway -> groups, on one shared subject.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupCommand {
    /// Only this group acts on it (None: every group).
    pub to: Option<GroupId>,
    #[serde(flatten)]
    pub kind: GroupCommandKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "c", rename_all = "snake_case")]
pub enum GroupCommandKind {
    /// Start simulating these new entities.
    Spawn { entities: Vec<NewEntity> },
    /// Hand these entities off: (entity, new group).
    Release { moves: Vec<(EntityId, GroupId)> },
    /// A player disconnected (sent to every group, so a late handoff or spawn is dropped too).
    Leave { id: EntityId },
}

/// An entity to create. `id` is 0 in a [`SpawnRequest`] (the orchestrator fills it in).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEntity {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub player: bool,
    pub kind: u16,
    /// Components on top of the kind's.
    #[serde(default)]
    pub parts: Vec<Part>,
}

// ---- Envelopes: anything aimed at something ----

/// The one message type for anything aimed at something: handoffs, players' inputs and
/// commands, and the game's actions. Code never picks a destination process; it picks an
/// address, and the bus delivers it to whoever owns that right now.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub to: Address,
    pub from: Facts,
    /// The process that put it on the bus this time (0: the gateway). Differs from
    /// `from.group` after a forward, or when a Spatial Layer Cell passes an area action on.
    pub via: u32,
    /// Per sender, for ordering and dedupe (with `from.id`).
    pub seq: u64,
    /// The same for every envelope caused by one action, for tracing it across processes.
    pub trace: u64,
    /// Times forwarded (it gives up after a few).
    pub hops: u8,
    pub kind: Kind,
    /// Who confirms it arrived, for sure delivery: the process that will send it again until
    /// it hears back. None: sent once, and lost if the bus drops it.
    #[serde(default)]
    pub ack: Option<AckTo>,
    /// Times it was sent again (0: the first time).
    #[serde(default)]
    pub resends: u8,
}

impl Envelope {
    /// What its receiver confirms it by: (sender, the sender's number for it).
    pub fn key(&self) -> (EntityId, u64) {
        (self.from.id, self.seq)
    }
}

/// The process waiting to hear that a sure envelope arrived.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AckTo {
    Group(GroupId),
    Spatial(GroupId),
    Gateway(u32),
}

impl AckTo {
    /// Where its confirmations go.
    pub fn subject(self) -> String {
        match self {
            AckTo::Group(id) => crate::subjects::group_acks_subject(id),
            AckTo::Spatial(id) => crate::subjects::spatial_acks_subject(id),
            AckTo::Gateway(id) => crate::subjects::gateway_acks_subject(id),
        }
    }
}

/// Confirmations, to the process that sent the envelopes: each listed one arrived. One
/// message per tick from each process that got something sure from it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Acks {
    /// The group or Spatial Layer Cell confirming.
    pub by: u32,
    /// Actions and commands, by [`Envelope::key`].
    #[serde(default)]
    pub envelopes: Vec<(EntityId, u64)>,
    /// Handoffs: (entity, the old group's number for it).
    #[serde(default)]
    pub handoffs: Vec<(EntityId, u64)>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "to", rename_all = "snake_case")]
pub enum Address {
    /// Whoever simulates this entity right now.
    Entity { id: EntityId },
    /// Every entity in the circle: the Spatial Layer Cells covering it pass it on to every
    /// group with an entity there, and each group delivers it to its own.
    Area { x: f32, y: f32, r: f32 },
    /// One group (handoffs, forwards).
    Group { id: GroupId },
    /// Every entity in every group.
    Everyone,
}

/// About the sender, stamped by a server (never taken from a client).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct Facts {
    pub id: EntityId,
    /// Group that stamped this (0: the gateway).
    pub group: GroupId,
    pub x: f32,
    pub y: f32,
    pub tick: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Kind {
    /// An entity moving to the receiving group (its full record, encoded by the group).
    Handoff {
        #[serde(with = "serde_bytes")]
        entity: Vec<u8>,
    },
    /// A player's input for sub-step `sub` of tick `at_tick` (from the gateway).
    Input {
        at_tick: u64,
        #[serde(default)]
        sub: u8,
        #[serde(with = "serde_bytes")]
        input: Vec<u8>,
    },
    /// A player's command (from the gateway), for their own entity only.
    Command {
        msg: u16,
        #[serde(with = "serde_bytes")]
        body: Vec<u8>,
    },
    /// The game's action, from one entity to its address.
    Action {
        msg: u16,
        #[serde(with = "serde_bytes")]
        body: Vec<u8>,
    },
}

// ---- The spatial layer ----

/// One entity as a group reports it to a Spatial Layer Cell.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reported {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub player: bool,
    pub kind: u16,
    /// Its visible components.
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// A game event and where it happened.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventAt {
    pub x: f32,
    pub y: f32,
    pub msg: u16,
    #[serde(with = "serde_bytes")]
    pub body: Vec<u8>,
    /// Players who get it wherever they are (besides everyone who can see (x, y)).
    #[serde(default)]
    pub involves: Vec<EntityId>,
}

/// Group -> Spatial Layer Cell, every tick (even when empty, so the cell knows the group is done).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupReport {
    pub group: GroupId,
    pub tick: u64,
    /// Every entity of this group standing in the cell's area, as of `tick`. Replaces what the
    /// cell had from this group, so an entity that walked out simply isn't listed.
    pub entities: Vec<Reported>,
    /// Spatial queries whose circle touches the cell's area.
    #[serde(default)]
    pub queries: Vec<SenseQuery>,
    /// What happened in the cell's area, for the players watching it.
    #[serde(default)]
    pub events: Vec<EventAt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SenseQuery {
    /// The entity asking.
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    /// Only the nearest this many (0: everyone in the circle).
    #[serde(default)]
    pub nearest: u16,
    /// Always include this one if it's in the circle.
    #[serde(default)]
    pub target: Option<EntityId>,
    /// Only these visible components in the answer (None: all of them).
    #[serde(default)]
    pub only: Option<Vec<u16>>,
}

/// Another entity, as a Spatial Layer Cell describes it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Seen {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    pub group: GroupId,
    #[serde(default)]
    pub player: bool,
    pub kind: u16,
    /// Its visible components (those the question asked for).
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// Spatial Layer Cell -> group: answers to last tick's questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenseAnswers {
    pub spatial: GroupId,
    /// The tick the positions are from.
    pub tick: u64,
    pub answers: Vec<SenseAnswer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenseAnswer {
    pub id: EntityId,
    pub seen: Vec<Seen>,
}

/// Each gateway -> every Spatial Layer Cell, every tick: its players and where they look.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewers {
    /// Which gateway: cells keep each gateway's players apart and send each its own views.
    #[serde(default)]
    pub gateway: u32,
    pub tick: u64,
    pub viewers: Vec<Viewer>,
}

/// A connected player and where their view is centred.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Viewer {
    pub player: EntityId,
    pub x: f32,
    pub y: f32,
}

/// One entity in a player's view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewEntity {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    pub group: GroupId,
    pub spatial: GroupId,
    #[serde(default)]
    pub player: bool,
    pub kind: u16,
    /// Its visible components.
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// Spatial Layer Cell -> one gateway, every tick: its players' views of the cell's area.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialViews {
    pub spatial: GroupId,
    pub tick: u64,
    pub views: Vec<CellView>,
}

/// One player's view of one Spatial Layer Cell's area.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CellView {
    pub player: EntityId,
    pub entities: Vec<ViewEntity>,
    #[serde(default)]
    pub events: Vec<WireEvent>,
    #[serde(default)]
    pub senses: Vec<SenseDebug>,
}

/// Every server process -> gateway, every tick: what it received and streamed (debug only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugReport {
    pub from: u32,
    pub tick: u64,
    #[serde(default)]
    pub bus: Vec<BusMsg>,
    #[serde(default)]
    pub streams: Vec<StreamMsg>,
}
