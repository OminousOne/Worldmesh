//! Server Entity Groups: each simulates a set of entities, wherever they stand.
//!
//! A group owns entities, not land: an entity walking anywhere stays in its group. Entities
//! only change groups when the orchestrator says so (scaling up or down, or regrouping), with
//! a live handoff: the entity's record (its data, pending inputs and recent history) travels
//! to the new group, and the old one forwards anything that still arrives for it.
//!
//! Each tick a group:
//! 1. delivers last tick's commands and actions (to an entity, an area or everyone) in a fixed
//!    order, to every handler registered for them,
//! 2. runs the systems, stage by stage, for every entity (with its player's inputs due this tick),
//! 3. reports its entities (and their questions, and events) to the Spatial Layer Cells whose
//!    areas they stand in, hands off what it was told to, and publishes its envelopes.
//!
//! One process (a "group host") runs any number of groups.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::info;

use crate::entity::{Entity, Meta, Record};
use crate::me::{Incoming, Me, Requests};
use crate::world::Rules;
use worldmesh_core::config::WorldConfig;
use worldmesh_core::protocol::{
    Address, Assignment, BusKind, BusMsg, DebugReport, EntityId, Envelope, EventAt, Facts,
    GroupCommand, GroupCommandKind, GroupId, GroupReport, Heartbeat, HostCommand, Kind, NewEntity,
    ProcessStats, Reported, Seen, SenseAnswers, SenseQuery, SpawnRequest, StreamKind, StreamMsg,
};
use worldmesh_core::subjects::{
    entity_subject, group_inbox_subject, group_senses_subject, random_process_id,
    spatial_inbox_subject, spatial_reports_subject, ASSIGNMENTS, BROADCAST, DEBUG_REPORTS,
    GROUP_COMMANDS, HEARTBEATS, HOST_COMMANDS, SPAWNS, STATS,
};
use worldmesh_core::{bus, stats, time};

/// How long the old group keeps forwarding envelopes for an entity it handed off.
const FORWARD_FOR_MS: u64 = 1000;
/// An entity forgets spatial query results this old.
const SENSE_TTL_MS: u64 = 1000;
/// Ticks of position history kept per entity (for [`Me::position_at`]).
const TRAIL_TICKS: usize = 20;
/// Envelopes per entity remembered, so one delivered twice (directly and forwarded) applies once.
const RECENT_DELIVERIES: usize = 32;
/// An envelope gives up after being forwarded this many times.
const MAX_HOPS: u8 = 3;

/// A player's inputs not simulated yet: (tick, sub-step) -> (sequence number, input).
type Schedule = BTreeMap<(u64, u8), (u64, Vec<u8>)>;
/// Inputs due this tick: (sub-step, input).
type Due = Vec<(u8, Vec<u8>)>;

/// What one group did since its last stats report.
#[derive(Default)]
pub struct Counters {
    /// Actions delivered from this group, from another group; sent to other groups; forwarded.
    pub actions_local: u64,
    pub actions_received: u64,
    pub actions_sent: u64,
    pub actions_forwarded: u64,
    pub handoffs: u64,
    pub queries: u64,
    /// Time spent ticking: total and worst (microseconds), and how many ticks.
    pub tick_us_sum: u64,
    pub tick_us_max: u64,
    pub ticks: u64,
}

/// One entity's latest spatial query results: (tick of the positions, what it saw).
type SenseRound = (u64, Vec<Seen>);

/// One Server Entity Group's world: its entities and everything in flight for them.
pub struct GroupWorld {
    pub group_id: GroupId,
    rules: Arc<Rules>,
    config: WorldConfig,
    /// Spatial Layer Cell owning each chunk. None until the first assignment arrives.
    owners: Option<Vec<GroupId>>,
    /// Spatial Layer Cells in use: every one gets a report every tick.
    spatial: Vec<GroupId>,
    /// Entities simulated here. A BTreeMap so iteration order is deterministic.
    entities: BTreeMap<EntityId, Record>,
    schedule: BTreeMap<EntityId, Schedule>,
    /// Players who disconnected. Late handoffs and spawns for them are dropped.
    left: BTreeSet<EntityId>,
    /// Entities handed off recently: (new group, until this tick). Envelopes for them are forwarded.
    moved: BTreeMap<EntityId, (GroupId, u64)>,
    /// Each entity's latest spatial query results: (tick of the positions, what it saw).
    senses: BTreeMap<EntityId, SenseRound>,
    /// Questions asked this tick, sent with the reports.
    queries: Vec<SenseQuery>,
    /// Entities the orchestrator asked us to hand off: (entity, new group).
    releases: Vec<(EntityId, GroupId)>,
    /// Envelopes from the bus, delivered on the next step.
    inbox: Vec<Envelope>,
    /// Actions sent here to entities here: delivered on the next step too, exactly like ones
    /// from other groups, so which group an entity is in never changes timing.
    local_next: Vec<Envelope>,
    /// (subject, envelope) to publish after the tick.
    outbox: Vec<(String, Envelope)>,
    /// The game's events this tick, and where.
    events: Vec<EventAt>,
    /// Entities the game spawned this tick, for the orchestrator to place.
    spawns: Vec<NewEntity>,
    bus_log: Vec<BusMsg>,
    pub counters: Counters,
}

impl GroupWorld {
    pub fn new(group_id: GroupId, rules: Arc<Rules>) -> Self {
        let config = rules.config;
        Self {
            group_id,
            rules,
            config,
            owners: None,
            spatial: Vec::new(),
            entities: BTreeMap::new(),
            schedule: BTreeMap::new(),
            left: BTreeSet::new(),
            moved: BTreeMap::new(),
            senses: BTreeMap::new(),
            queries: Vec::new(),
            releases: Vec::new(),
            inbox: Vec::new(),
            local_next: Vec::new(),
            outbox: Vec::new(),
            events: Vec::new(),
            spawns: Vec::new(),
            bus_log: Vec::new(),
            counters: Counters::default(),
        }
    }

    pub fn set_assignment(&mut self, assignment: &Assignment) {
        self.owners = Some(assignment.owners.clone());
        self.spatial = assignment.spatial.clone();
    }

    /// The Spatial Layer Cell whose area (x, y) is in.
    fn spatial_at(&self, x: f32, y: f32) -> Option<GroupId> {
        self.owners
            .as_ref()
            .and_then(|o| o.get(self.config.chunk_at(x, y)).copied())
            .filter(|&s| s != 0)
    }

    /// Every Spatial Layer Cell whose area the circle touches.
    fn spatial_in_circle(&self, x: f32, y: f32, r: f32) -> BTreeSet<GroupId> {
        let Some(owners) = &self.owners else {
            return BTreeSet::new();
        };
        self.config
            .chunks_in_circle(x, y, r)
            .into_iter()
            .filter_map(|i| owners.get(i).copied())
            .filter(|&s| s != 0)
            .collect()
    }

    /// Entity subjects this group must listen to: what it simulates, plus what it handed off
    /// recently (to forward strays).
    pub fn listening_entities(&self) -> BTreeSet<EntityId> {
        self.entities
            .keys()
            .chain(self.moved.keys())
            .copied()
            .collect()
    }

    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Every entity this group simulates.
    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values().map(|r| &r.entity)
    }

    /// An entity simulated here (for tests and tools).
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(&id).map(|r| &r.entity)
    }

    /// A command from the orchestrator (or, for leaves, the gateway).
    pub fn command(&mut self, cmd: GroupCommand) {
        if cmd.to.is_some_and(|to| to != self.group_id) {
            return;
        }
        match cmd.kind {
            GroupCommandKind::Spawn { entities } => {
                for n in entities {
                    if self.left.contains(&n.id) || self.entities.contains_key(&n.id) {
                        continue;
                    }
                    let Some(entity) = self
                        .rules
                        .new_entity(n.id, n.x, n.y, n.player, n.kind, &n.parts)
                    else {
                        tracing::warn!(
                            id = n.id,
                            kind = n.kind,
                            "couldn't create an entity (unknown kind or component)"
                        );
                        continue;
                    };
                    if n.player {
                        info!(id = n.id, x = n.x, y = n.y, "player joined");
                    }
                    self.entities.insert(
                        n.id,
                        Record {
                            entity,
                            meta: Meta::default(),
                        },
                    );
                }
            }
            GroupCommandKind::Release { moves } => {
                for (id, to) in moves {
                    let queued = self.releases.iter().any(|(r, _)| *r == id);
                    if to != self.group_id && self.entities.contains_key(&id) && !queued {
                        self.releases.push((id, to));
                    }
                }
            }
            GroupCommandKind::Leave { id } => {
                if self.entities.remove(&id).is_some() {
                    info!(id, "player left");
                }
                self.schedule.remove(&id);
                self.senses.remove(&id);
                self.releases.retain(|(r, _)| *r != id);
                self.left.insert(id);
            }
        }
    }

    /// An envelope from the bus (an entity subject, this group's inbox, or the broadcast).
    pub fn receive(&mut self, env: Envelope) {
        let kind = match (&env.kind, env.to) {
            _ if env.hops > 0 => BusKind::Forward,
            (Kind::Handoff { .. }, _) => BusKind::Handoff,
            (Kind::Input { .. }, _) => BusKind::Input,
            (Kind::Command { .. }, _) => BusKind::Command,
            (Kind::Action { .. }, Address::Area { .. }) => BusKind::Area,
            (Kind::Action { .. }, Address::Everyone) => BusKind::Broadcast,
            (Kind::Action { .. }, _) => BusKind::Action,
        };
        self.bus_log.push(BusMsg {
            from: env.via,
            to: self.group_id,
            kind,
            trace: env.trace,
        });
        match env.kind {
            // Inputs are scheduled right away: they say exactly which tick they belong to.
            Kind::Input {
                at_tick,
                sub,
                ref input,
            } => {
                if let Address::Entity { id } = env.to {
                    if self.entities.contains_key(&id) {
                        let input = input.clone();
                        self.schedule_input(id, at_tick, sub, env.seq, input);
                    } else {
                        self.forward(id, env);
                    }
                }
            }
            Kind::Handoff { ref entity } => {
                let bytes = entity.clone();
                self.receive_handoff(&bytes);
            }
            Kind::Command { .. } | Kind::Action { .. } => self.inbox.push(env),
        }
    }

    /// Takes effect at that sub-step of `at_tick`, or at the start of the next tick if that
    /// tick has already been simulated. Of two inputs for the same moment, the later-sent wins.
    fn schedule_input(&mut self, id: EntityId, at_tick: u64, sub: u8, seq: u64, input: Vec<u8>) {
        let key = (
            at_tick,
            sub.min(self.config.input_substeps.saturating_sub(1)),
        );
        let entry = self.schedule.entry(id).or_default();
        if entry.get(&key).is_none_or(|(s, _)| seq >= *s) {
            entry.insert(key, (seq, input));
        }
    }

    fn receive_handoff(&mut self, bytes: &[u8]) {
        let Some(mut record) = Record::decode(&self.rules, bytes) else {
            return;
        };
        let id = record.entity.id;
        if self.left.contains(&id) {
            return;
        }
        for (at_tick, sub, seq, input) in std::mem::take(&mut record.meta.pending) {
            self.schedule_input(id, at_tick, sub, seq, input.into_vec());
        }
        self.moved.remove(&id);
        self.entities.insert(id, record);
    }

    /// Answers from a Spatial Layer Cell. A circle crossing two cells' areas gets two answers
    /// for the same tick; they add up.
    pub fn receive_senses(&mut self, answers: SenseAnswers) {
        for answer in answers.answers {
            if !self.entities.contains_key(&answer.id) {
                continue;
            }
            let seen = answer.seen;
            let entry = self.senses.entry(answer.id).or_insert((0, Vec::new()));
            if answers.tick > entry.0 {
                *entry = (answers.tick, seen);
            } else if answers.tick == entry.0 {
                // Cells answer about disjoint areas, so duplicates are rare (an entity mid-way
                // between owners); skip them without comparing every pair.
                let known: BTreeSet<EntityId> = entry.1.iter().map(|o| o.id).collect();
                entry
                    .1
                    .extend(seen.into_iter().filter(|s| !known.contains(&s.id)));
            }
        }
    }

    /// Every player's inputs due during `tick`, in order. Inputs meant for ticks already
    /// simulated (they arrived late) take effect at the start of this one.
    fn take_due_inputs(&mut self, tick: u64) -> BTreeMap<EntityId, Due> {
        let mut due = BTreeMap::new();
        for (&id, inputs) in self.schedule.iter_mut() {
            let later = inputs.split_off(&(tick + 1, 0));
            let now = std::mem::replace(inputs, later);
            let list: Due = now
                .into_iter()
                .map(|((at, sub), (_, bytes))| (if at < tick { 0 } else { sub }, bytes))
                .collect();
            if !list.is_empty() {
                due.insert(id, list);
            }
        }
        self.schedule.retain(|_, inputs| !inputs.is_empty());
        due
    }

    /// Advance to world tick `tick`.
    pub fn step(&mut self, tick: u64) {
        self.deliver_envelopes(tick);
        self.tick_entities(tick);
        self.moved.retain(|_, &mut (_, until)| until >= tick);
        let ttl = time::ticks_in(SENSE_TTL_MS);
        self.senses.retain(|_, (t, _)| *t + ttl >= tick);
    }

    /// Run `f` on entity `id` with a [`Me`] for it, then carry out what game code asked for.
    /// False if the entity isn't here (or was despawned).
    fn with_me(
        &mut self,
        id: EntityId,
        tick: u64,
        trace: u64,
        inputs: &[(u8, Vec<u8>)],
        f: impl FnOnce(&mut Me<'_>),
    ) -> bool {
        let Some(mut record) = self.entities.remove(&id) else {
            return false;
        };
        let rules = self.rules.clone();
        let mut requests = Requests::default();
        let mut despawned = false;
        {
            let sensed: &[Seen] = match self.senses.get(&id) {
                Some((_, s)) => s,
                None => &[],
            };
            let Record { entity, meta } = &mut record;
            let mut me = Me {
                rules: &rules,
                entity,
                meta,
                tick,
                group: self.group_id,
                trace,
                others: &self.entities,
                sensed,
                inputs,
                requests: &mut requests,
                despawned: &mut despawned,
            };
            f(&mut me);
        }
        if despawned {
            self.schedule.remove(&id);
            self.senses.remove(&id);
            self.releases.retain(|(r, _)| *r != id);
        } else {
            self.entities.insert(id, record);
        }
        self.carry_out(requests);
        !despawned
    }

    /// What game code asked for during a callback.
    fn carry_out(&mut self, requests: Requests) {
        for env in requests.envelopes {
            self.route(env);
        }
        self.events.extend(requests.events);
        self.spawns.extend(requests.spawns);
        self.counters.queries += requests.queries.len() as u64;
        self.queries.extend(requests.queries);
    }

    /// Every entity simulates this tick once: each stage's systems run for every entity before
    /// the next stage starts, with its player's inputs due this tick. An entity that arrived by
    /// handoff already simulated for this tick (the previous group finished it first) is not
    /// simulated again.
    fn tick_entities(&mut self, tick: u64) {
        let mut due = self.take_due_inputs(tick);
        let ids: Vec<EntityId> = self
            .entities
            .iter()
            .filter(|(_, r)| r.meta.last_tick < tick)
            .map(|(&id, _)| id)
            .collect();
        let rules = self.rules.clone();
        for stage in &rules.systems {
            for &id in &ids {
                let inputs = due.get(&id).map(Vec::as_slice).unwrap_or(&[]);
                for (only, system) in stage {
                    let Some(record) = self.entities.get(&id) else {
                        break;
                    }; // despawned meanwhile
                    if only.is_some_and(|t| !record.entity.has_type(t)) {
                        continue;
                    }
                    self.with_me(id, tick, 0, inputs, |me| system(me));
                }
            }
        }
        for id in ids {
            due.remove(&id);
            let Some(record) = self.entities.get_mut(&id) else {
                continue;
            };
            record.meta.last_tick = tick;
            let (x, y) = (record.entity.x, record.entity.y);
            record.meta.trail.push((tick, x, y));
            if record.meta.trail.len() > TRAIL_TICKS {
                record.meta.trail.remove(0);
            }
        }
    }

    // ---- The router ----

    /// Deliver an envelope to whoever owns its address. Anything for this group waits for the
    /// next step, exactly like envelopes from other groups.
    fn route(&mut self, env: Envelope) {
        match env.to {
            Address::Entity { id } => {
                if self.entities.contains_key(&id) {
                    self.local_next.push(env);
                } else {
                    self.counters.actions_sent += 1;
                    self.outbox.push((entity_subject(id), env));
                }
            }
            // Even our own entities in the circle hear about it through the spatial layer:
            // only it knows who stands there.
            Address::Area { x, y, r } => {
                for spatial in self.spatial_in_circle(x, y, r) {
                    self.counters.actions_sent += 1;
                    self.outbox
                        .push((spatial_inbox_subject(spatial), env.clone()));
                }
            }
            Address::Everyone => {
                self.counters.actions_sent += 1;
                self.outbox.push((BROADCAST.to_string(), env));
            }
            Address::Group { id } => self.outbox.push((group_inbox_subject(id), env)),
        }
    }

    /// An envelope for an entity that isn't here: pass it to the new group if it just left.
    fn forward(&mut self, id: EntityId, mut env: Envelope) {
        let Some(&(owner, _)) = self.moved.get(&id) else {
            return;
        }; // gone for good
        if env.hops >= MAX_HOPS {
            return;
        }
        self.counters.actions_forwarded += 1;
        env.hops += 1;
        env.via = self.group_id;
        self.outbox.push((group_inbox_subject(owner), env));
    }

    /// Envelopes arrived since last tick, in a fixed order: (tick sent, sender, number).
    /// Anything sent on this tick or later waits: groups tick independently, so another group's
    /// tick-N action can arrive before we run tick N, and it must still land on N + 1.
    fn deliver_envelopes(&mut self, tick: u64) {
        let mut envs = std::mem::take(&mut self.inbox);
        envs.append(&mut self.local_next);
        let (mut envs, later): (Vec<_>, Vec<_>) =
            envs.into_iter().partition(|e| e.from.tick < tick);
        self.inbox = later;
        envs.sort_by_key(|e| (e.from.tick, e.from.id, e.seq));
        for env in envs {
            let incoming = |area: Option<(f32, f32, f32)>, everyone: bool| Incoming {
                from: env.from,
                area,
                everyone,
                forwarded: env.hops > 0,
                trace: env.trace,
            };
            match (&env.to, &env.kind) {
                (&Address::Entity { id }, Kind::Command { msg, body }) => {
                    // Only an entity's own player may command it.
                    if !self.entities.contains_key(&id) {
                        self.forward(id, env.clone());
                        continue;
                    }
                    if env.from.id != id || !self.first_delivery(id, &env.from, env.seq) {
                        continue;
                    }
                    let rules = self.rules.clone();
                    let Some(info) = rules.message(*msg) else {
                        continue;
                    };
                    let Some(command) = (info.decode)(body) else {
                        continue;
                    };
                    self.with_me(id, tick, env.trace, &[], |me| {
                        for handler in &info.commands {
                            if me.despawned() {
                                break;
                            }
                            handler(me, &*command);
                        }
                    });
                }
                (&Address::Entity { id }, Kind::Action { msg, body }) => {
                    if !self.entities.contains_key(&id) {
                        self.forward(id, env.clone());
                        continue;
                    }
                    if !self.first_delivery(id, &env.from, env.seq) {
                        continue; // a duplicate (old owner forwarded what we also got directly)
                    }
                    self.count_delivery(&env.from);
                    let from = incoming(None, false);
                    self.act(id, tick, *msg, body, &from);
                }
                (&Address::Area { x, y, r }, Kind::Action { msg, body }) => {
                    let inside: Vec<EntityId> = self
                        .entities
                        .values()
                        .filter(|rec| {
                            rec.entity.id != env.from.id
                                && distance(x, y, rec.entity.x, rec.entity.y) <= r
                        })
                        .map(|rec| rec.entity.id)
                        .collect();
                    let from = incoming(Some((x, y, r)), false);
                    for id in inside {
                        // Two Spatial Layer Cells may both pass on the same area action.
                        if self.first_delivery(id, &env.from, env.seq) {
                            self.count_delivery(&env.from);
                            self.act(id, tick, *msg, body, &from);
                        }
                    }
                }
                (Address::Everyone, Kind::Action { msg, body }) => {
                    let ids: Vec<EntityId> = self.entities.keys().copied().collect();
                    let from = incoming(None, true);
                    for id in ids {
                        if self.entities.contains_key(&id)
                            && self.first_delivery(id, &env.from, env.seq)
                        {
                            self.count_delivery(&env.from);
                            self.act(id, tick, *msg, body, &from);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Run every handler registered for action `msg` on entity `id`, in order.
    fn act(&mut self, id: EntityId, tick: u64, msg: u16, body: &[u8], from: &Incoming) {
        let rules = self.rules.clone();
        let Some(info) = rules.message(msg) else {
            return;
        };
        let Some(action) = (info.decode)(body) else {
            return;
        };
        self.with_me(id, tick, from.trace, &[], |me| {
            for handler in &info.actions {
                if me.despawned() {
                    break;
                }
                handler(me, &*action, from);
            }
        });
    }

    fn count_delivery(&mut self, from: &Facts) {
        if from.group == self.group_id {
            self.counters.actions_local += 1;
        } else {
            self.counters.actions_received += 1;
        }
    }

    /// Record who sent it on the entity; false if it was already delivered.
    fn first_delivery(&mut self, id: EntityId, from: &Facts, seq: u64) -> bool {
        let Some(record) = self.entities.get_mut(&id) else {
            return false;
        };
        let key = (from.id, seq, from.group == 0);
        let recent = &mut record.meta.recent;
        if recent.contains(&key) {
            return false;
        }
        recent.push(key);
        if recent.len() > RECENT_DELIVERIES {
            recent.remove(0);
        }
        true
    }

    // ---- What goes out after a tick ----

    /// Envelopes to publish: (subject, envelope).
    pub fn take_outbox(&mut self) -> Vec<(String, Envelope)> {
        std::mem::take(&mut self.outbox)
    }

    /// Entities the game spawned, for the orchestrator to place.
    pub fn take_spawns(&mut self) -> Vec<NewEntity> {
        std::mem::take(&mut self.spawns)
    }

    /// One report per Spatial Layer Cell in use (empty ones too, so each knows we're done):
    /// our entities in its area, our questions touching it, what happened in it.
    pub fn take_reports(&mut self, tick: u64) -> Vec<(GroupId, GroupReport)> {
        let group = self.group_id;
        let mut reports: BTreeMap<GroupId, GroupReport> = self
            .spatial
            .iter()
            .map(|&s| {
                (
                    s,
                    GroupReport {
                        group,
                        tick,
                        entities: Vec::new(),
                        queries: Vec::new(),
                        events: Vec::new(),
                    },
                )
            })
            .collect();
        for record in self.entities.values() {
            let e = &record.entity;
            let Some(report) = self.spatial_at(e.x, e.y).and_then(|s| reports.get_mut(&s)) else {
                continue;
            };
            let Some(parts) = self.rules.encode(e.components.iter(), true) else {
                continue;
            };
            report.entities.push(Reported {
                id: e.id,
                x: e.x,
                y: e.y,
                player: e.player,
                kind: e.kind,
                parts,
            });
        }
        for q in std::mem::take(&mut self.queries) {
            for s in self.spatial_in_circle(q.x, q.y, q.r) {
                if let Some(report) = reports.get_mut(&s) {
                    report.queries.push(q.clone());
                }
            }
        }
        for ev in std::mem::take(&mut self.events) {
            if let Some(report) = self
                .spatial_at(ev.x, ev.y)
                .and_then(|s| reports.get_mut(&s))
            {
                report.events.push(ev);
            }
        }
        reports.into_iter().collect()
    }

    /// Remove and return the handoff envelope for every entity the orchestrator asked us to
    /// release. Its pending inputs go with it; we keep listening a while to forward strays.
    pub fn take_releases(&mut self, tick: u64) -> Vec<(GroupId, Envelope)> {
        let mut out = Vec::new();
        for (id, to) in std::mem::take(&mut self.releases) {
            let Some(mut record) = self.entities.remove(&id) else {
                continue;
            };
            if let Some(schedule) = self.schedule.remove(&id) {
                record.meta.pending = schedule
                    .into_iter()
                    .map(|((t, sub), (seq, input))| {
                        (t, sub, seq, serde_bytes::ByteBuf::from(input))
                    })
                    .collect();
            }
            self.senses.remove(&id);
            self.moved
                .insert(id, (to, tick + time::ticks_in(FORWARD_FOR_MS)));
            self.counters.handoffs += 1;
            let e = &record.entity;
            let from = Facts {
                id,
                group: self.group_id,
                x: e.x,
                y: e.y,
                tick: record.meta.last_tick,
            };
            let Some(bytes) = record.encode(&self.rules) else {
                continue;
            };
            let env = Envelope {
                to: Address::Group { id: to },
                from,
                via: self.group_id,
                seq: 0,
                trace: 0,
                hops: 0,
                kind: Kind::Handoff { entity: bytes },
            };
            out.push((to, env));
        }
        out
    }

    pub fn heartbeat(&self) -> Heartbeat {
        Heartbeat {
            role: "group".into(),
            id: self.group_id,
            entities: self
                .entities
                .values()
                .map(|r| (r.entity.id, r.entity.x, r.entity.y))
                .collect(),
            ..Heartbeat::default()
        }
    }

    pub fn take_bus_log(&mut self) -> Vec<BusMsg> {
        std::mem::take(&mut self.bus_log)
    }

    pub fn record_tick(&mut self, took: Duration) {
        let us = took.as_micros() as u64;
        self.counters.tick_us_sum += us;
        self.counters.tick_us_max = self.counters.tick_us_max.max(us);
        self.counters.ticks += 1;
    }

    /// This group's numbers over the last `secs` seconds (then they start again from zero).
    pub fn take_stats(&mut self, secs: f32) -> ProcessStats {
        let c = std::mem::take(&mut self.counters);
        let per_sec = |n: u64| n as f32 / secs;
        ProcessStats {
            role: "group".into(),
            id: self.group_id,
            players: self.entities.len() as u32,
            tick_ms_avg: (c.ticks > 0).then(|| c.tick_us_sum as f32 / c.ticks as f32 / 1000.0),
            tick_ms_max: (c.ticks > 0).then(|| c.tick_us_max as f32 / 1000.0),
            load_pct: (c.ticks > 0).then(|| stats::load_pct(c.tick_us_sum, c.ticks)),
            actions_local: per_sec(c.actions_local),
            actions_received: per_sec(c.actions_received),
            actions_sent: per_sec(c.actions_sent),
            actions_forwarded: per_sec(c.actions_forwarded),
            handoffs: per_sec(c.handoffs),
            queries: per_sec(c.queries),
            ..ProcessStats::default()
        }
    }
}

fn distance(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
}

// ---- Fake lag (debug) ----

/// Something a group received, held back while it has fake lag.
enum Inbound {
    Envelope(Envelope),
    Senses(SenseAnswers),
    Command(GroupCommand),
}

/// Fake network lag for one group: what arrives is handed over `lag` after it arrived, in
/// arrival order.
#[derive(Default)]
struct LagQueue {
    held: VecDeque<(Instant, Inbound)>,
}

impl LagQueue {
    fn push(&mut self, at: Instant, item: Inbound) {
        // Keep arrival order (messages from different subjects are gathered one subject at a time).
        let i = self
            .held
            .iter()
            .rposition(|(t, _)| *t <= at)
            .map_or(0, |i| i + 1);
        self.held.insert(i, (at, item));
    }

    /// Everything that's been held for at least `lag` by `now`, oldest first.
    fn take_due(&mut self, now: Instant, lag: Duration) -> Vec<Inbound> {
        let mut due = Vec::new();
        while self.held.front().is_some_and(|(at, _)| *at + lag <= now) {
            due.push(self.held.pop_front().expect("checked").1);
        }
        due
    }
}

// ---- The process ----

/// One group in a host process: its world, its subscriptions, and its fake lag.
struct Runner {
    world: GroupWorld,
    inbox: UnboundedReceiver<(Instant, Envelope)>,
    broadcast: UnboundedReceiver<(Instant, Envelope)>,
    senses: UnboundedReceiver<(Instant, SenseAnswers)>,
    /// One subject per entity it simulates (or just handed off); dropping a receiver unsubscribes.
    entity_subs: BTreeMap<EntityId, UnboundedReceiver<(Instant, Envelope)>>,
    /// Fake network lag (debug): everything this group receives and sends is held back this long.
    lag: Duration,
    received: LagQueue,
    /// Messages to send later: (when, subject, encoded). A task sends them in order.
    delayed: UnboundedSender<(Instant, String, Vec<u8>)>,
    /// When the last delayed message is due (later messages never overtake it).
    last_due: Instant,
}

impl Runner {
    async fn start(
        nats: &async_nats::Client,
        id: GroupId,
        rules: Arc<Rules>,
        assignment: Option<&Assignment>,
    ) -> Result<Self> {
        let mut world = GroupWorld::new(id, rules);
        let mut lag = Duration::ZERO;
        if let Some(a) = assignment {
            world.set_assignment(a);
            lag = lag_of(a, id);
        }
        let inbox = bus::subscribe_timed::<Envelope>(nats, &group_inbox_subject(id)).await?;
        let broadcast = bus::subscribe_timed::<Envelope>(nats, BROADCAST).await?;
        let senses = bus::subscribe_timed::<SenseAnswers>(nats, &group_senses_subject(id)).await?;
        bus::publish(nats, HEARTBEATS.to_string(), &world.heartbeat()).await;
        // Sends held-back messages when they're due; ends when the runner is dropped.
        let (delayed, mut due) =
            tokio::sync::mpsc::unbounded_channel::<(Instant, String, Vec<u8>)>();
        let sender = nats.clone();
        tokio::spawn(async move {
            while let Some((at, subject, payload)) = due.recv().await {
                tokio::time::sleep_until(at.into()).await;
                bus::publish_bytes(&sender, subject, payload).await;
            }
        });
        info!(group = format!("{id:04x}"), "Server Entity Group started");
        Ok(Self {
            world,
            inbox,
            broadcast,
            senses,
            entity_subs: BTreeMap::new(),
            lag,
            received: LagQueue::default(),
            delayed,
            last_due: Instant::now(),
        })
    }

    fn lagging(&self) -> bool {
        !self.lag.is_zero() || !self.received.held.is_empty() || Instant::now() < self.last_due
    }

    /// A command from the orchestrator (spawn, release, leave), held back like everything else.
    fn command(&mut self, at: Instant, cmd: GroupCommand) {
        if self.lagging() {
            self.received.push(at, Inbound::Command(cmd));
        } else {
            self.world.command(cmd);
        }
    }

    /// Send now, or after the fake lag.
    async fn send(&mut self, nats: &async_nats::Client, subject: String, payload: Vec<u8>) {
        let now = Instant::now();
        if self.lag.is_zero() && now >= self.last_due {
            bus::publish_bytes(nats, subject, payload).await;
            return;
        }
        let at = (now + self.lag).max(self.last_due);
        self.last_due = at;
        let _ = self.delayed.send((at, subject, payload));
    }

    /// One tick of this group: take in what arrived, simulate, publish. Debug traffic goes into
    /// `bus_log` and `streams`, sent once for the whole process.
    async fn tick(
        &mut self,
        nats: &async_nats::Client,
        tick: u64,
        bus_log: &mut Vec<BusMsg>,
        streams: &mut Vec<StreamMsg>,
    ) -> Result<()> {
        let started = Instant::now();
        let id = self.world.group_id;
        let mut arrived: Vec<(Instant, Inbound)> = Vec::new();
        while let Ok((at, env)) = self.inbox.try_recv() {
            arrived.push((at, Inbound::Envelope(env)));
        }
        while let Ok((at, env)) = self.broadcast.try_recv() {
            arrived.push((at, Inbound::Envelope(env)));
        }
        for rx in self.entity_subs.values_mut() {
            while let Ok((at, env)) = rx.try_recv() {
                arrived.push((at, Inbound::Envelope(env)));
            }
        }
        while let Ok((at, answers)) = self.senses.try_recv() {
            arrived.push((at, Inbound::Senses(answers)));
        }
        let ready: Vec<Inbound> = if self.lagging() {
            for (at, item) in arrived {
                self.received.push(at, item);
            }
            self.received.take_due(started, self.lag)
        } else {
            arrived.into_iter().map(|(_, item)| item).collect()
        };
        for item in ready {
            match item {
                Inbound::Envelope(env) => self.world.receive(env),
                Inbound::Senses(answers) => self.world.receive_senses(answers),
                Inbound::Command(cmd) => self.world.command(cmd),
            }
        }

        self.world.step(tick);

        // Reports first: an entity handed off this tick is still in this tick's report, and its
        // new group reports it from the next tick, so it never vanishes in between.
        let mut out: Vec<(String, Option<Vec<u8>>)> = Vec::new();
        for (spatial, report) in self.world.take_reports(tick) {
            streams.push(StreamMsg {
                from: id,
                to: spatial,
                kind: StreamKind::Report,
                n: report.entities.len() as u32,
            });
            out.push((spatial_reports_subject(spatial), bus::encode_now(&report)));
        }
        for (to, env) in self.world.take_releases(tick) {
            out.push((group_inbox_subject(to), bus::encode_now(&env)));
        }
        for (subject, env) in self.world.take_outbox() {
            out.push((subject, bus::encode_now(&env)));
        }
        let spawns = self.world.take_spawns();
        if !spawns.is_empty() {
            out.push((
                SPAWNS.to_string(),
                bus::encode_now(&SpawnRequest {
                    from_group: id,
                    entities: spawns,
                }),
            ));
        }
        for (subject, payload) in out {
            if let Some(payload) = payload {
                self.send(nats, subject, payload).await;
            }
        }
        bus_log.extend(self.world.take_bus_log());

        // Listen to exactly the entity subjects this group is responsible for now.
        let wanted = self.world.listening_entities();
        self.entity_subs.retain(|e, _| wanted.contains(e));
        for e in wanted {
            if let std::collections::btree_map::Entry::Vacant(slot) = self.entity_subs.entry(e) {
                slot.insert(bus::subscribe_timed(nats, &entity_subject(e)).await?);
            }
        }
        // Heartbeats aren't held back: they're how the orchestrator knows the group is alive,
        // and a slow link isn't a dead process.
        if tick.is_multiple_of(time::ticks_in(500)) {
            bus::publish(nats, HEARTBEATS.to_string(), &self.world.heartbeat()).await;
        }
        self.world.record_tick(started.elapsed());
        Ok(())
    }
}

/// This group's fake lag in the assignment.
fn lag_of(a: &Assignment, group: GroupId) -> Duration {
    let ms = a
        .group_lag
        .iter()
        .find(|(g, _)| *g == group)
        .map_or(0, |(_, ms)| *ms);
    Duration::from_millis(u64::from(ms))
}

/// A group host process: runs its first group, then whatever groups the orchestrator starts here.
pub async fn run(
    nats: async_nats::Client,
    rules: Arc<Rules>,
    first_group: Option<GroupId>,
) -> Result<()> {
    let host_id = random_process_id(false);
    let first = first_group
        .filter(|&id| id != 0 && !worldmesh_core::subjects::is_spatial_id(id))
        .unwrap_or_else(|| random_process_id(false));
    let mut assignment: Option<Assignment> = None;
    let mut runners: BTreeMap<GroupId, Runner> = BTreeMap::new();
    runners.insert(
        first,
        Runner::start(&nats, first, rules.clone(), None).await?,
    );

    let mut assignments = bus::subscribe::<Assignment>(&nats, ASSIGNMENTS).await?;
    let mut commands = bus::subscribe_timed::<GroupCommand>(&nats, GROUP_COMMANDS).await?;
    let mut hosts = bus::subscribe::<HostCommand>(&nats, HOST_COMMANDS).await?;
    stats::spawn_reporter(nats.clone(), "group-host", host_id);
    info!(
        process = format!("{host_id:04x}"),
        tick_ms = time::tick_ms(),
        "group process running"
    );
    let heartbeat = |runners: &BTreeMap<GroupId, Runner>| Heartbeat {
        role: "host".into(),
        id: host_id,
        groups: runners.keys().copied().collect(),
        ..Heartbeat::default()
    };
    bus::publish(&nats, HEARTBEATS.to_string(), &heartbeat(&runners)).await;
    let mut last_stats = Instant::now();

    loop {
        tokio::time::sleep(time::until_next_tick()).await;
        let tick = time::current_tick();
        let started = Instant::now();

        while let Ok(a) = assignments.try_recv() {
            bus::set_binary(a.binary_bus);
            for (id, r) in runners.iter_mut() {
                r.world.set_assignment(&a);
                let lag = lag_of(&a, *id);
                if lag != r.lag {
                    info!(
                        group = format!("{id:04x}"),
                        ms = lag.as_millis() as u64,
                        "fake lag"
                    );
                    r.lag = lag;
                }
            }
            assignment = Some(a);
        }
        while let Ok(cmd) = hosts.try_recv() {
            if cmd.host != host_id {
                continue;
            }
            for id in cmd.start {
                if let std::collections::btree_map::Entry::Vacant(slot) = runners.entry(id) {
                    slot.insert(
                        Runner::start(&nats, id, rules.clone(), assignment.as_ref()).await?,
                    );
                }
            }
            for id in cmd.stop {
                if runners.remove(&id).is_some() {
                    info!(group = format!("{id:04x}"), "Server Entity Group stopped");
                }
            }
            bus::publish(&nats, HEARTBEATS.to_string(), &heartbeat(&runners)).await;
        }
        while let Ok((at, cmd)) = commands.try_recv() {
            for r in runners.values_mut() {
                r.command(at, cmd.clone());
            }
        }

        let (mut bus_log, mut streams) = (Vec::new(), Vec::new());
        for r in runners.values_mut() {
            r.tick(&nats, tick, &mut bus_log, &mut streams).await?;
        }
        let report = DebugReport {
            from: host_id,
            tick,
            bus: bus_log,
            streams,
        };
        bus::publish(&nats, DEBUG_REPORTS.to_string(), &report).await;

        if tick.is_multiple_of(time::ticks_in(500)) {
            bus::publish(&nats, HEARTBEATS.to_string(), &heartbeat(&runners)).await;
        }
        // Each group's own numbers, once a second (the process's CPU and memory come from the
        // reporter spawned above).
        let secs = last_stats.elapsed().as_secs_f32();
        if secs >= 1.0 {
            last_stats = Instant::now();
            for r in runners.values_mut() {
                bus::publish(&nats, STATS.to_string(), &r.world.take_stats(secs)).await;
            }
        }
        stats::PLAYERS.store(
            runners
                .values()
                .map(|r| r.world.entity_count() as u64)
                .sum(),
            Relaxed,
        );
        stats::record_tick(started.elapsed());
    }
}

#[cfg(test)]
pub(crate) mod tests;
