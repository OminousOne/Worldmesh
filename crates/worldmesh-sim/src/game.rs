//! What a game implements ([`Game`]) and what its code gets to work with ([`Me`]).
//!
//! A game never deals with processes, groups, cells or bytes. It says what its entities are,
//! what happens to them each tick, and what its actions do when they land. Worldmesh runs that
//! code in whichever Server Entity Group simulates each entity, and delivers every action to
//! its target wherever the target is.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use worldmesh_core::config::WorldConfig;
use worldmesh_core::protocol::{Address, EntityId, Envelope, Facts, GroupId, Kind, SenseQuery};

/// Bounds every game type needs: it travels between processes.
pub trait Wire: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {}
impl<T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> Wire for T {}

/// Your game. Implement this and pass it to `worldmesh::run`.
///
/// Every server process of a world runs the same binary, so they all have the same `Game`.
/// Only group hosts call the simulation methods; the gateway calls [`Game::welcome`] and
/// [`Game::join`]; every role calls [`Game::config`].
pub trait Game: Sized + Send + Sync + 'static {
    /// An entity's state: whatever your game needs (HP, inventory, AI state). It's simulated
    /// by one group at a time and travels with the entity when it moves to another group.
    type Data: Wire;
    /// What others (players' clients, and other entities' "what's near me?") see of an
    /// entity. Keep it small: it goes to every client that can see the entity, and is resent
    /// whenever it changes.
    type View: Wire;
    /// A player's input (movement, say). Timed: it takes effect at an exact sub-step of an
    /// exact tick, on the server and in the client's prediction alike.
    type Input: Wire;
    /// A player's discrete request ("use this ability on that"), handled by their own entity.
    type Command: Wire;
    /// What entities do to each other. Sent with [`Me::send`] to an entity, an area or
    /// everyone; handled by [`Game::on_action`] on the next tick, wherever the target is.
    type Action: Wire;
    /// Something to show players near where it happened (a hit, a sound).
    type Event: Wire;
    /// Sent to every player when they connect (a map, say).
    type Welcome: Serialize + Send + Sync + 'static;

    /// The world's size, chunks and view. Must be the same in every process.
    fn config(&self) -> WorldConfig;

    /// What every player gets on connect.
    fn welcome(&self) -> Self::Welcome;

    /// A player connected: where their entity appears, and its starting data.
    fn join(&self, player: EntityId) -> Spawn<Self::Data>;

    /// What others see of `entity`.
    fn view(&self, entity: &Entity<Self::Data>) -> Self::View;

    /// Once per tick per entity: move it, think for it, whatever your game does.
    /// `inputs` are the player's inputs due during this tick, in order (empty for entities no
    /// player controls). Each says at which sub-step of the tick it takes effect.
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Self::Input>]);

    /// The entity's own player sent a command.
    fn on_command(&self, me: &mut Me<'_, Self>, command: Self::Command) {
        let _ = (me, command);
    }

    /// An action reached this entity. `from` says who sent it, from where and on which tick,
    /// stamped by the sender's group (never by a client), so you can trust it for checks like
    /// range.
    fn on_action(&self, me: &mut Me<'_, Self>, action: Self::Action, from: &Incoming) {
        let _ = (me, action, from);
    }
}

/// An entity: what Worldmesh needs to know (id, position, whether a player controls it) plus
/// your game's data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(bound(serialize = "D: Serialize", deserialize = "D: DeserializeOwned"))]
pub struct Entity<D> {
    pub id: EntityId,
    /// Position in tiles. Where it stands decides which Spatial Layer Cell reports it and who
    /// can see it; move it freely.
    pub x: f32,
    pub y: f32,
    /// Controlled by a connected player (its id is the player's).
    pub player: bool,
    pub data: D,
}

/// Where a new entity appears and what it starts with.
#[derive(Debug, Clone)]
pub struct Spawn<D> {
    pub x: f32,
    pub y: f32,
    pub data: D,
}

/// A player's input and the sub-step of the tick it takes effect at
/// (0 to [`WorldConfig::input_substeps`] - 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Timed<I> {
    pub sub: u8,
    pub input: I,
}

/// Where to send an action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum To {
    /// One entity, wherever it is.
    Entity(EntityId),
    /// Every entity within `r` tiles of (x, y), except the sender.
    Area { x: f32, y: f32, r: f32 },
    /// Every entity in the world (use sparingly).
    Everyone,
}

/// How an action arrived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Incoming {
    /// The sender, where it stood and the tick it sent this, stamped by its group.
    pub from: Facts,
    /// Sent to an area this entity stands in, rather than to it directly.
    pub area: Option<(f32, f32, f32)>,
    /// Sent to everyone.
    pub everyone: bool,
    /// Passed on by this entity's previous group after a handoff.
    pub forwarded: bool,
    /// The same for everything one action led to (for tracing it in the debug view).
    pub trace: u64,
}

/// Another entity, as the spatial layer described it in answer to [`Me::query`].
#[derive(Debug, Clone, PartialEq)]
pub struct Sensed<V> {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    /// The group simulating it.
    pub group: GroupId,
    pub player: bool,
    pub view: V,
}

/// An entity near this one: from this group (exact, current) or from the latest answer to
/// [`Me::query`] (a tick or two old).
#[derive(Debug, Clone, PartialEq)]
pub struct Nearby<V> {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    pub group: GroupId,
    pub player: bool,
    pub view: V,
    /// In this entity's own group (so exact and current).
    pub local: bool,
}

/// What Worldmesh keeps about an entity besides your game's data. Travels on handoffs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Meta {
    /// The tick it was last simulated on, so the next group never simulates a tick twice.
    pub last_tick: u64,
    /// Numbers the envelopes it sends, so receivers never apply one twice.
    pub seq: u64,
    /// (sender, number, sent by the gateway) of the last envelopes delivered to it. A player's
    /// commands (numbered by the gateway) and their entity's actions (numbered by the entity)
    /// can share a number, hence the flag.
    pub recent: Vec<(EntityId, u64, bool)>,
    /// Where it stood at the end of recent ticks: (tick, x, y).
    pub trail: Vec<(u64, f32, f32)>,
    /// A player's inputs not simulated yet: (tick, sub-step, sequence number, input). Only the
    /// owning group hears a player's inputs, so they travel with the entity on a handoff.
    pub pending: Vec<(u64, u8, u64, serde_bytes::ByteBuf)>,
}

/// An entity and its bookkeeping, as simulated by a group and sent on a handoff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(bound(serialize = "D: Serialize", deserialize = "D: DeserializeOwned"))]
pub(crate) struct Record<D> {
    pub entity: Entity<D>,
    pub meta: Meta,
}

/// Things the game asked for during a callback, carried out by the group afterwards.
pub(crate) struct Requests<G: Game> {
    pub envelopes: Vec<Envelope>,
    pub events: Vec<(f32, f32, G::Event, Vec<EntityId>)>,
    pub spawns: Vec<Spawn<G::Data>>,
    pub queries: Vec<SenseQuery>,
}

impl<G: Game> Default for Requests<G> {
    fn default() -> Self {
        Self { envelopes: Vec::new(), events: Vec::new(), spawns: Vec::new(), queries: Vec::new() }
    }
}

/// The entity being simulated, and everything the game can do from it.
///
/// It dereferences to the [`Entity`], so `me.x`, `me.data.hp` and so on work directly.
pub struct Me<'a, G: Game> {
    pub(crate) game: &'a G,
    pub(crate) entity: &'a mut Entity<G::Data>,
    pub(crate) meta: &'a mut Meta,
    pub(crate) tick: u64,
    pub(crate) group: GroupId,
    pub(crate) trace: u64,
    pub(crate) others: &'a BTreeMap<EntityId, Record<G::Data>>,
    pub(crate) sensed: &'a [Sensed<G::View>],
    pub(crate) requests: &'a mut Requests<G>,
    pub(crate) despawned: &'a mut bool,
}

impl<G: Game> std::ops::Deref for Me<'_, G> {
    type Target = Entity<G::Data>;
    fn deref(&self) -> &Self::Target {
        self.entity
    }
}

impl<G: Game> std::ops::DerefMut for Me<'_, G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.entity
    }
}

impl<G: Game> Me<'_, G> {
    /// The tick being simulated.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The Server Entity Group simulating this entity right now.
    pub fn group(&self) -> GroupId {
        self.group
    }

    /// The trace of what's being handled: the incoming action's or command's, or 0 in
    /// [`Game::tick`] (where each [`Me::send`] starts a new trace).
    pub fn trace(&self) -> u64 {
        self.trace
    }

    /// Send an action. It's delivered on the next tick wherever the target is (even if it's
    /// on another machine, or moving between groups), stamped with this entity's id, position
    /// and the current tick. Returns its trace (to tag events with, for the debug view).
    pub fn send(&mut self, to: To, action: G::Action) -> u64 {
        let Some(bytes) = worldmesh_core::codec::to_bytes(&action) else { return 0 };
        self.meta.seq += 1;
        let from = Facts { id: self.entity.id, group: self.group, x: self.entity.x, y: self.entity.y, tick: self.tick };
        let trace = if self.trace != 0 { self.trace } else { worldmesh_core::codec::trace_id(self.entity.id, self.meta.seq) };
        let to = match to {
            To::Entity(id) => Address::Entity { id },
            To::Area { x, y, r } => Address::Area { x, y, r },
            To::Everyone => Address::Everyone,
        };
        let env = Envelope { to, from, via: self.group, seq: self.meta.seq, trace, hops: 0, kind: Kind::Action { action: bytes } };
        self.requests.envelopes.push(env);
        trace
    }

    /// Show an event to the players who can see this entity's position (and to this entity's
    /// own player, wherever they're looking).
    pub fn emit(&mut self, event: G::Event) {
        let (x, y, id) = (self.entity.x, self.entity.y, self.entity.id);
        self.requests.events.push((x, y, event, vec![id]));
    }

    /// Show an event to the players who can see (x, y).
    pub fn emit_at(&mut self, x: f32, y: f32, event: G::Event) {
        self.requests.events.push((x, y, event, Vec::new()));
    }

    /// Like [`Me::emit`], and also to these players wherever they are (say, whoever caused
    /// it, so they see the result of their action even if it landed off their screen).
    pub fn emit_involving(&mut self, event: G::Event, players: &[EntityId]) {
        let (x, y, id) = (self.entity.x, self.entity.y, self.entity.id);
        let mut involves = players.to_vec();
        involves.push(id);
        self.requests.events.push((x, y, event, involves));
    }

    /// Ask the spatial layer who's within `r` tiles of this entity, in any group. The answer
    /// arrives by the next tick in [`Me::sensed`] (and [`Me::nearby`]).
    pub fn query(&mut self, r: f32) {
        self.query_with(r, 0, None);
    }

    /// Like [`Me::query`], but only the `nearest` closest (0: everyone in range), always
    /// including `target` if it's in range. A hook for pruning big answers.
    pub fn query_with(&mut self, r: f32, nearest: u16, target: Option<EntityId>) {
        let q = SenseQuery { id: self.entity.id, x: self.entity.x, y: self.entity.y, r, nearest, target };
        self.requests.queries.push(q);
    }

    /// Other groups' entities from the latest answer to [`Me::query`] (dropped after a second).
    pub fn sensed(&self) -> &[Sensed<G::View>] {
        self.sensed
    }

    /// The other entities in this entity's group: exact and current.
    pub fn local(&self) -> impl Iterator<Item = &Entity<G::Data>> {
        self.others.values().map(|r| &r.entity)
    }

    /// One entity of this group by id.
    pub fn local_entity(&self, id: EntityId) -> Option<&Entity<G::Data>> {
        self.others.get(&id).map(|r| &r.entity)
    }

    /// Every entity within `r` tiles that this entity knows of: its own group's (exact) and
    /// other groups' from the latest answer to [`Me::query`].
    pub fn nearby(&self, r: f32) -> Vec<Nearby<G::View>> {
        let (x, y) = (self.entity.x, self.entity.y);
        let within = |ox: f32, oy: f32| ((ox - x).powi(2) + (oy - y).powi(2)).sqrt() <= r;
        let mut out: Vec<Nearby<G::View>> = self
            .local()
            .filter(|e| within(e.x, e.y))
            .map(|e| Nearby { id: e.id, x: e.x, y: e.y, group: self.group, player: e.player, view: self.game.view(e), local: true })
            .collect();
        for s in self.sensed {
            if !self.others.contains_key(&s.id) && s.id != self.entity.id && within(s.x, s.y) {
                out.push(Nearby { id: s.id, x: s.x, y: s.y, group: s.group, player: s.player, view: s.view.clone(), local: false });
            }
        }
        out
    }

    /// What this entity knows of entity `id`: exact if it's in this group, otherwise from the
    /// latest answer to [`Me::query`]. None if it knows nothing.
    pub fn find(&self, id: EntityId) -> Option<Nearby<G::View>> {
        if let Some(e) = self.local_entity(id) {
            return Some(Nearby { id, x: e.x, y: e.y, group: self.group, player: e.player, view: self.game.view(e), local: true });
        }
        self.sensed.iter().find(|s| s.id == id).map(|s| Nearby {
            id,
            x: s.x,
            y: s.y,
            group: s.group,
            player: s.player,
            view: s.view.clone(),
            local: false,
        })
    }

    /// Where this entity stood at the end of tick `tick` (its current position if that's
    /// older than its recent trail). Use it to judge an action by where the target was when
    /// it was sent, not where it is when it lands.
    pub fn position_at(&self, tick: u64) -> (f32, f32) {
        self.meta.trail.iter().rev().find(|(t, ..)| *t <= tick).map(|&(_, x, y)| (x, y)).unwrap_or((self.entity.x, self.entity.y))
    }

    /// Create a new entity. The orchestrator gives it an id and a group (by where it stands);
    /// it starts ticking within a tick or two.
    pub fn spawn(&mut self, x: f32, y: f32, data: G::Data) {
        self.requests.spawns.push(Spawn { x, y, data });
    }

    /// Remove this entity after this callback. (A player's entity comes back: their client
    /// is re-joined. Use it for entities no player controls.)
    pub fn despawn(&mut self) {
        *self.despawned = true;
    }
}
