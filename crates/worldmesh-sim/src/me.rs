//! `Me`: the entity being simulated, and everything game code can do from it.
//!
//! Game code only ever changes the entity it runs for. Everything else it learns by reading
//! (its own group's entities exactly, others from "what's near me?" answers, both limited to
//! visible components) and changes by sending actions, which land on the next tick wherever
//! the target is. That keeps a game behaving the same however its entities are split between
//! groups.

use std::any::TypeId;
use std::collections::BTreeMap;

use worldmesh_core::codec;
use worldmesh_core::config::WorldConfig;
use worldmesh_core::protocol::{
    Address, EntityId, Envelope, EventAt, Facts, GroupId, Kind, NewEntity, Part, Seen, SenseQuery,
};

use crate::component::{Component, Message, Wire};
use crate::entity::{Entity, Meta, Record, Spawn};
use crate::world::{Category, Rules};

/// Who an action goes to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum To {
    /// One entity, wherever it is.
    Entity(EntityId),
    /// Every entity within `r` tiles of (x, y), except the sender.
    Area { x: f32, y: f32, r: f32 },
    /// Every entity in the world, except the sender.
    Everyone,
}

/// An input and the sub-step of its tick it takes effect at (0 to `input_substeps - 1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Timed<I> {
    pub sub: u8,
    pub input: I,
}

/// How an action reached this entity. `from` is stamped by the sender's group from the
/// entity it simulates, never by a client, so it can be trusted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Incoming {
    /// The sender, where it stood and the tick it sent this.
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

/// A "what's near me?" question. `me.query(6.0)` is the same as `me.query(Query::within(6.0))`.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    r: f32,
    nearest: u16,
    target: Option<EntityId>,
    only: Option<Vec<TypeId>>,
}

impl Query {
    /// Everyone within `r` tiles, with all their visible components.
    pub fn within(r: f32) -> Self {
        Self {
            r,
            nearest: 0,
            target: None,
            only: None,
        }
    }

    /// Only the `n` nearest (plus the target, if one is set).
    pub fn nearest(mut self, n: u16) -> Self {
        self.nearest = n;
        self
    }

    /// Always include this entity if it's in range.
    pub fn target(mut self, id: EntityId) -> Self {
        self.target = Some(id);
        self
    }

    /// Only this visible component in the answer (call again for more). Answers are the
    /// biggest thing on the bus in a crowd: ask only for what you'll read.
    pub fn with<T: Component>(mut self) -> Self {
        self.only
            .get_or_insert_with(Vec::new)
            .push(TypeId::of::<T>());
        self
    }
}

impl From<f32> for Query {
    fn from(r: f32) -> Self {
        Query::within(r)
    }
}

/// Where a [`Nearby`] came from.
#[derive(Clone, Copy)]
enum Source<'a> {
    Local(&'a Entity),
    Sensed(&'a [Part]),
}

/// Another entity, as this one knows it: from its own group (exact, current) or from the
/// latest answer to [`Me::query`] (a tick or two old). Either way only its visible
/// components can be read.
#[derive(Clone, Copy)]
pub struct Nearby<'a> {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    pub group: GroupId,
    pub player: bool,
    /// In this entity's own group (so exact and current).
    pub local: bool,
    kind: u16,
    source: Source<'a>,
    rules: &'a Rules,
}

impl<'a> Nearby<'a> {
    pub fn kind(&self) -> &'a str {
        self.rules.kind_name(self.kind)
    }

    /// Its `T`, if `T` is visible and it has one (and, from an answer, if the question asked
    /// for it).
    pub fn get<T: Component>(&self) -> Option<T> {
        let id = self.rules.component_id::<T>()?;
        if !self.rules.component(id)?.visible {
            return None;
        }
        match self.source {
            Source::Local(e) => e.get::<T>().cloned(),
            Source::Sensed(parts) => parts
                .iter()
                .find(|(c, _)| *c == id)
                .and_then(|(_, b)| codec::from_bytes(b)),
        }
    }

    pub fn has<T: Component>(&self) -> bool {
        self.get::<T>().is_some()
    }

    pub fn distance_to(&self, x: f32, y: f32) -> f32 {
        ((self.x - x).powi(2) + (self.y - y).powi(2)).sqrt()
    }
}

impl std::fmt::Debug for Nearby<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nearby")
            .field("id", &self.id)
            .field("x", &self.x)
            .field("y", &self.y)
            .field("kind", &self.kind())
            .field("group", &self.group)
            .field("local", &self.local)
            .finish()
    }
}

/// Things game code asked for during a callback, carried out by the group afterwards.
#[derive(Default)]
pub(crate) struct Requests {
    pub envelopes: Vec<Envelope>,
    pub events: Vec<EventAt>,
    pub spawns: Vec<NewEntity>,
    pub queries: Vec<SenseQuery>,
}

/// The entity being simulated, and everything game code can do from it.
///
/// It dereferences to the [`Entity`]: `me.id`, `me.x`, `me.get::<Health>()`,
/// `me.get_mut::<Health>()` work directly, and `me.x = ...` moves it.
pub struct Me<'a> {
    pub(crate) rules: &'a Rules,
    pub(crate) entity: &'a mut Entity,
    pub(crate) meta: &'a mut Meta,
    pub(crate) tick: u64,
    pub(crate) group: GroupId,
    pub(crate) trace: u64,
    pub(crate) others: &'a BTreeMap<EntityId, Record>,
    pub(crate) sensed: &'a [Seen],
    pub(crate) inputs: &'a [(u8, Vec<u8>)],
    pub(crate) requests: &'a mut Requests,
    pub(crate) despawned: &'a mut bool,
}

impl std::ops::Deref for Me<'_> {
    type Target = Entity;
    fn deref(&self) -> &Entity {
        self.entity
    }
}

impl std::ops::DerefMut for Me<'_> {
    fn deref_mut(&mut self) -> &mut Entity {
        self.entity
    }
}

impl<'a> Me<'a> {
    /// The tick being simulated.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The world's size, view and input sub-steps.
    pub fn config(&self) -> WorldConfig {
        self.rules.config
    }

    /// The Server Entity Group simulating this entity right now.
    pub fn group(&self) -> GroupId {
        self.group
    }

    /// The trace of what's being handled: the incoming command's or action's, or 0 in a
    /// system (where each [`Me::send`] starts a new trace).
    pub fn trace(&self) -> u64 {
        self.trace
    }

    /// This entity's kind.
    pub fn kind(&self) -> &str {
        self.rules.kind_name(self.entity.kind)
    }

    /// Has this entity been despawned during this callback?
    pub fn despawned(&self) -> bool {
        *self.despawned
    }

    /// Where this entity stood at the end of tick `tick` (its current position if that's
    /// older than its recent trail). Judge an action by where the target was when it was
    /// sent, not where it is when it lands.
    pub fn position_at(&self, tick: u64) -> (f32, f32) {
        self.meta
            .trail
            .iter()
            .rev()
            .find(|(t, ..)| *t <= tick)
            .map(|&(_, x, y)| (x, y))
            .unwrap_or((self.entity.x, self.entity.y))
    }

    /// This entity's player's inputs due this tick, in order, each with the sub-step it takes
    /// effect at. Empty outside systems, and for entities no player controls.
    pub fn inputs<I: Wire>(&self) -> Vec<Timed<I>> {
        self.inputs
            .iter()
            .filter_map(|(sub, bytes)| {
                Some(Timed {
                    sub: *sub,
                    input: codec::from_bytes(bytes)?,
                })
            })
            .collect()
    }

    /// Add a component to this entity (replacing one of the same type).
    pub fn insert<T: Component>(&mut self, component: T) {
        if self.rules.component_id::<T>().is_none() {
            tracing::error!(
                component = T::NAME,
                "inserting a component that isn't registered: add world.component::<..>()"
            );
            return;
        }
        self.entity.put(Box::new(component));
    }

    /// Remove a component from this entity, returning it.
    pub fn remove<T: Component>(&mut self) -> Option<T> {
        self.entity.take::<T>()
    }

    fn message_id<M: Message>(&self, category: Category) -> Option<u16> {
        let id = self.rules.message_id::<M>().filter(|&id| {
            self.rules
                .message(id)
                .is_some_and(|m| m.category == category)
        });
        if id.is_none() {
            tracing::error!(
                message = M::NAME,
                ?category,
                "not registered as a {category:?}: register it on the world"
            );
        }
        id
    }

    /// Send an action. It's delivered on the next tick wherever the target is (even on
    /// another machine, or moving between groups), exactly once, stamped with this entity's
    /// id, position and the current tick. Returns its trace (to tag events with).
    pub fn send<A: Message>(&mut self, to: To, action: A) -> u64 {
        let Some(msg) = self.message_id::<A>(Category::Action) else {
            return 0;
        };
        let Some(body) = codec::to_bytes(&action) else {
            return 0;
        };
        self.meta.seq += 1;
        let from = Facts {
            id: self.entity.id,
            group: self.group,
            x: self.entity.x,
            y: self.entity.y,
            tick: self.tick,
        };
        let trace = if self.trace != 0 {
            self.trace
        } else {
            codec::trace_id(self.entity.id, self.meta.seq)
        };
        let to = match to {
            To::Entity(id) => Address::Entity { id },
            To::Area { x, y, r } => Address::Area { x, y, r },
            To::Everyone => Address::Everyone,
        };
        let env = Envelope {
            to,
            from,
            via: self.group,
            seq: self.meta.seq,
            trace,
            hops: 0,
            kind: Kind::Action { msg, body },
        };
        self.requests.envelopes.push(env);
        trace
    }

    fn push_event<E: Message>(&mut self, x: f32, y: f32, event: E, involves: Vec<EntityId>) {
        let Some(msg) = self.message_id::<E>(Category::Event) else {
            return;
        };
        if let Some(body) = codec::to_bytes(&event) {
            self.requests.events.push(EventAt {
                x,
                y,
                msg,
                body,
                involves,
            });
        }
    }

    /// Show an event to the players who can see this entity (and to its own player).
    pub fn emit<E: Message>(&mut self, event: E) {
        let (x, y, id) = (self.entity.x, self.entity.y, self.entity.id);
        self.push_event(x, y, event, vec![id]);
    }

    /// Show an event to the players who can see (x, y).
    pub fn emit_at<E: Message>(&mut self, x: f32, y: f32, event: E) {
        self.push_event(x, y, event, Vec::new());
    }

    /// Like [`Me::emit`], and also to these players wherever they are (say, whoever caused
    /// it, so they see the result even if it landed off their screen).
    pub fn emit_involving<E: Message>(&mut self, event: E, players: &[EntityId]) {
        let (x, y, id) = (self.entity.x, self.entity.y, self.entity.id);
        let mut involves = players.to_vec();
        involves.push(id);
        self.push_event(x, y, event, involves);
    }

    /// Ask the spatial layer who's near, in any group. The answer arrives by the next tick
    /// (see [`Me::sensed`] and [`Me::nearby`]).
    pub fn query(&mut self, query: impl Into<Query>) {
        let q = query.into();
        let only = q.only.map(|types| {
            types
                .iter()
                .filter_map(|t| {
                    let id = self.rules.component_id_of(*t);
                    if id.is_none_or(|id| !self.rules.component(id).is_some_and(|c| c.visible)) {
                        tracing::error!("a query asked for a component that isn't visible: register it with world.visible::<..>()");
                    }
                    id
                })
                .collect()
        });
        let (id, x, y) = (self.entity.id, self.entity.x, self.entity.y);
        self.requests.queries.push(SenseQuery {
            id,
            x,
            y,
            r: q.r,
            nearest: q.nearest,
            target: q.target,
            only,
        });
    }

    fn nearby_local(&self, e: &'a Entity) -> Nearby<'a> {
        Nearby {
            id: e.id,
            x: e.x,
            y: e.y,
            group: self.group,
            player: e.player,
            local: true,
            kind: e.kind,
            source: Source::Local(e),
            rules: self.rules,
        }
    }

    fn nearby_sensed(&self, s: &'a Seen) -> Nearby<'a> {
        Nearby {
            id: s.id,
            x: s.x,
            y: s.y,
            group: s.group,
            player: s.player,
            local: false,
            kind: s.kind,
            source: Source::Sensed(&s.parts),
            rules: self.rules,
        }
    }

    /// The other entities in this entity's group: exact and current.
    pub fn local(&self) -> impl Iterator<Item = Nearby<'a>> + '_ {
        let others: &'a BTreeMap<EntityId, Record> = self.others;
        others.values().map(move |r| self.nearby_local(&r.entity))
    }

    /// Other groups' entities from the latest answer to [`Me::query`] (dropped after a second).
    pub fn sensed(&self) -> impl Iterator<Item = Nearby<'a>> + '_ {
        let sensed: &'a [Seen] = self.sensed;
        sensed
            .iter()
            .filter(move |s| !self.others.contains_key(&s.id) && s.id != self.entity.id)
            .map(move |s| self.nearby_sensed(s))
    }

    /// Every entity within `r` tiles that this entity knows of: its own group's (exact) and
    /// other groups' from the latest answer.
    pub fn nearby(&self, r: f32) -> Vec<Nearby<'a>> {
        let (x, y) = (self.entity.x, self.entity.y);
        self.local()
            .chain(self.sensed())
            .filter(|n| n.distance_to(x, y) <= r)
            .collect()
    }

    /// What this entity knows of entity `id`: exact if it's in this group, otherwise from the
    /// latest answer. None if it knows nothing.
    pub fn find(&self, id: EntityId) -> Option<Nearby<'a>> {
        let others: &'a BTreeMap<EntityId, Record> = self.others;
        if let Some(r) = others.get(&id) {
            return Some(self.nearby_local(&r.entity));
        }
        let sensed: &'a [Seen] = self.sensed;
        sensed
            .iter()
            .find(|s| s.id == id)
            .map(|s| self.nearby_sensed(s))
    }

    /// Create an entity. The orchestrator gives it an id and a group; it starts ticking within
    /// a tick or two.
    pub fn spawn(&mut self, spawn: Spawn) {
        if let Some((kind, parts)) = self.rules.encode_spawn(&spawn) {
            self.requests.spawns.push(NewEntity {
                id: 0,
                x: spawn.x,
                y: spawn.y,
                player: false,
                kind,
                parts,
            });
        }
    }

    /// Remove this entity after this callback (handlers and systems after this one don't run
    /// for it). A player's entity comes back: their client is re-joined.
    pub fn despawn(&mut self) {
        *self.despawned = true;
    }
}
