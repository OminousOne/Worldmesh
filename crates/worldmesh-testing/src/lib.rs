//! Test your game's rules in one process, without NATS, tick by tick.
//!
//! A [`TestWorld`] runs real Server Entity Groups and Spatial Layer Cells and passes their
//! messages between them exactly as the bus would (actions to entities wherever they are,
//! area actions through the cells, handoffs, spatial query results, confirmations of sure
//! deliveries), but synchronously: one [`TestWorld::step`] is one tick of the whole world.
//! [`TestWorld::lose`] makes it drop some of them, like an overloaded bus.
//!
//! ```ignore
//! let mut w = TestWorld::new(my_world(), 2, 2); // 2 groups, 2 cells
//! let alice = w.join(1);
//! let bob = w.join_in(w.groups()[1], 2);
//! w.command(alice, Attack { target: bob });
//! w.step(); // the command runs and sends the attack
//! w.step(); // the attack lands, in bob's group
//! assert!(w.entity(bob).unwrap().get::<Health>().unwrap().0 < 100);
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Serialize;
use worldmesh_core::config::{partition, WorldConfig};
use worldmesh_core::protocol::{
    AckTo, Acks, Assignment, EntityId, Envelope, GroupCommand, GroupCommandKind, GroupId, Kind,
    NewEntity, SpatialViews, Viewer, Viewers, WireEvent, SPAWNED_ID_START,
};
use worldmesh_core::reliable::{Target, Unconfirmed};
use worldmesh_core::subjects::{BROADCAST, SPATIAL_ID_BIT};
use worldmesh_core::views::{from_player, merge_view};
use worldmesh_core::{codec, time};
use worldmesh_sim::group::GroupWorld;
use worldmesh_sim::world::{Category, Rules, World};
use worldmesh_sim::{Component, Entity, Message, Spawn};
use worldmesh_spatial::SpatialWorld;

/// The tick a test world starts on.
pub const FIRST_TICK: u64 = 1000;

/// What a player can see of an entity, as their client would get it: its visible components.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub id: EntityId,
    pub x: f32,
    pub y: f32,
    pub group: GroupId,
    pub spatial: GroupId,
    pub player: bool,
    /// Its kind's name.
    pub kind: String,
    /// Its visible components: (name, MessagePack).
    pub components: Vec<(String, Vec<u8>)>,
}

impl Seen {
    /// One of its visible components, decoded.
    pub fn get<T: Component>(&self) -> Option<T> {
        self.components
            .iter()
            .find(|(n, _)| n == T::NAME)
            .and_then(|(_, b)| codec::from_bytes(b))
    }

    pub fn has<T: Component>(&self) -> bool {
        self.components.iter().any(|(n, _)| n == T::NAME)
    }
}

/// The one gateway every TestWorld player is connected to.
const GATEWAY: u32 = 1;

/// Something on the (pretend) bus.
#[derive(Debug, Clone)]
enum Msg {
    Envelope(Envelope),
    Acks(Acks),
}

/// A whole world in one process. See the [module docs](self).
pub struct TestWorld {
    rules: Arc<Rules>,
    config: WorldConfig,
    tick: u64,
    groups: BTreeMap<GroupId, GroupWorld>,
    cells: BTreeMap<GroupId, SpatialWorld>,
    /// Messages waiting to be delivered before the next tick: (subject, message).
    pending: Vec<(String, Msg)>,
    /// Players' commands, and each player's latest input, until their group confirms them,
    /// as the gateway keeps them.
    sure: Unconfirmed,
    /// The % of messages dropped on the way, and how many draws so far (for a fixed sequence).
    loss: u8,
    draws: u64,
    /// Messages dropped so far.
    dropped: u64,
    next_spawn_id: EntityId,
    /// The game's events from the last step, in the order they happened.
    events: Vec<WireEvent>,
    /// Every cell's views from the last step.
    views: Vec<SpatialViews>,
    viewers: BTreeSet<EntityId>,
}

impl TestWorld {
    /// A world with `groups` Server Entity Groups (ids 1, 2, ...) and `cells` Spatial Layer
    /// Cells splitting the map along the Hilbert curve, like the orchestrator does.
    pub fn new(world: World, groups: u32, cells: u32) -> Self {
        let rules = world.build();
        let config = rules.config;
        let group_ids: Vec<GroupId> = (1..=groups.max(1)).collect();
        let cell_ids: Vec<GroupId> = (1..=cells.max(1)).map(|i| i | SPATIAL_ID_BIT).collect();
        let count = config.chunk_count();
        let assignment = Assignment {
            version: 1,
            chunk_size: config.chunk_size,
            chunks_x: config.chunks_x(),
            chunks_y: config.chunks_y(),
            owners: partition(
                &config.chunks_in_hilbert_order(),
                &vec![1.0; count],
                &cell_ids,
            ),
            spatial: cell_ids.clone(),
            groups: group_ids.clone(),
            spatial_running: cell_ids.len() as u32,
            groups_running: group_ids.len() as u32,
            group_hosts: 1,
            binary_bus: true,
            group_lag: Vec::new(),
            fake_loss: 0,
        };
        let groups = group_ids
            .iter()
            .map(|&id| {
                let mut g = GroupWorld::new(id, rules.clone());
                g.set_assignment(&assignment);
                (id, g)
            })
            .collect();
        let cells = cell_ids
            .iter()
            .map(|&id| {
                let mut c = SpatialWorld::new(id, config);
                c.set_assignment(&assignment);
                (id, c)
            })
            .collect();
        Self {
            rules,
            config,
            tick: FIRST_TICK,
            groups,
            cells,
            pending: Vec::new(),
            sure: Unconfirmed::default(),
            loss: 0,
            draws: 0,
            dropped: 0,
            next_spawn_id: SPAWNED_ID_START,
            events: Vec::new(),
            views: Vec::new(),
            viewers: BTreeSet::new(),
        }
    }

    /// Drop `pct` % of the messages between processes (envelopes and confirmations), in a
    /// fixed pseudo-random sequence, like an overloaded bus. Throwaway actions that are dropped
    /// are lost; sure ones ([`worldmesh_sim::Me::send_reliable`], players' commands and latest
    /// inputs, handoffs) are sent again until confirmed.
    pub fn lose(&mut self, pct: u8) -> &mut Self {
        self.loss = pct.min(100);
        self
    }

    /// Messages dropped so far by [`TestWorld::lose`].
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Sure envelopes (actions, commands, latest inputs, handoffs, area actions passed on)
    /// still waiting for a confirmation, everywhere.
    pub fn unconfirmed(&self) -> usize {
        self.sure.len()
            + self.groups.values().map(|g| g.unconfirmed()).sum::<usize>()
            + self.cells.values().map(|c| c.unconfirmed()).sum::<usize>()
    }

    /// The world's rules (its components, messages and kinds).
    pub fn rules(&self) -> &Arc<Rules> {
        &self.rules
    }

    pub fn config(&self) -> WorldConfig {
        self.config
    }

    /// The last tick simulated (or [`FIRST_TICK`] before the first step).
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The groups' ids.
    pub fn groups(&self) -> Vec<GroupId> {
        self.groups.keys().copied().collect()
    }

    /// Player `player` connects: their entity appears where and as what the world's join
    /// function says, in the first group. They also start getting views (see
    /// [`TestWorld::view`]).
    pub fn join(&mut self, player: EntityId) -> EntityId {
        let first = *self.groups.keys().next().expect("at least one group");
        self.join_in(first, player)
    }

    /// Like [`TestWorld::join`], in a chosen group.
    pub fn join_in(&mut self, group: GroupId, player: EntityId) -> EntityId {
        let spawn = self.rules.join(player);
        let (kind, parts) = self
            .rules
            .encode_spawn(&spawn)
            .expect("the join function's kind is registered");
        self.place(
            group,
            NewEntity {
                id: player,
                x: spawn.x,
                y: spawn.y,
                player: true,
                kind,
                parts,
            },
        );
        self.viewers.insert(player);
        player
    }

    /// Put an entity no player controls in `group`. Returns its id.
    pub fn spawn_in(&mut self, group: GroupId, spawn: Spawn) -> EntityId {
        let id = self.next_spawn_id;
        self.next_spawn_id += 1;
        let (kind, parts) = self.rules.encode_spawn(&spawn).expect("a registered kind");
        self.place(
            group,
            NewEntity {
                id,
                x: spawn.x,
                y: spawn.y,
                player: false,
                kind,
                parts,
            },
        );
        id
    }

    fn place(&mut self, group: GroupId, entity: NewEntity) {
        let g = self.groups.get_mut(&group).expect("a group of this world");
        g.command(GroupCommand {
            to: Some(group),
            kind: GroupCommandKind::Spawn {
                entities: vec![entity],
            },
        });
    }

    /// A player disconnects: their entity is removed wherever it is.
    pub fn leave(&mut self, player: EntityId) {
        self.viewers.remove(&player);
        for g in self.groups.values_mut() {
            g.command(GroupCommand {
                to: None,
                kind: GroupCommandKind::Leave { id: player },
            });
        }
    }

    /// A player's input, for sub-step `sub` of the next tick (as if sent just in time).
    pub fn input(&mut self, player: EntityId, sub: u8, input: impl Serialize) {
        let tick = self.tick + 1;
        self.input_at(player, tick, sub, input);
    }

    /// A player's input for sub-step `sub` of tick `tick`.
    pub fn input_at(&mut self, player: EntityId, tick: u64, sub: u8, input: impl Serialize) {
        let input = codec::to_bytes(&input).expect("the input encodes");
        let mut env = from_player(
            player,
            Kind::Input {
                at_tick: tick,
                sub,
                input,
            },
            false,
        );
        env.from.tick = self.tick;
        // As the gateway does: sure, and replacing any earlier input still unconfirmed.
        env.ack = Some(AckTo::Gateway(GATEWAY));
        self.sure
            .retain(|e| !(e.to == env.to && matches!(e.kind, Kind::Input { .. })));
        self.sure
            .track(&env, vec![Target::entity(player)], self.tick);
        self.pending.push((
            worldmesh_core::subjects::entity_subject(player),
            Msg::Envelope(env),
        ));
    }

    /// A player's command, handled by their entity on the next tick. Panics if `C` isn't
    /// registered as a command.
    pub fn command<C: Message>(&mut self, player: EntityId, command: C) {
        let msg = self
            .rules
            .message_id::<C>()
            .unwrap_or_else(|| panic!("{} isn't registered", C::NAME));
        assert_eq!(
            self.rules.message(msg).map(|m| m.category),
            Some(Category::Command),
            "{} isn't a command",
            C::NAME
        );
        let body = codec::to_bytes(&command).expect("the command encodes");
        let mut env = from_player(player, Kind::Command { msg, body }, true);
        env.from.tick = self.tick;
        env.ack = Some(AckTo::Gateway(GATEWAY));
        self.sure
            .track(&env, vec![Target::entity(player)], self.tick);
        self.pending.push((
            worldmesh_core::subjects::entity_subject(player),
            Msg::Envelope(env),
        ));
    }

    /// Move an entity to another group, as the orchestrator would: it's handed off live at
    /// the end of the next step.
    pub fn hand_off(&mut self, id: EntityId, to: GroupId) {
        if let Some(from) = self.group_of(id) {
            let g = self.groups.get_mut(&from).expect("listed");
            g.command(GroupCommand {
                to: Some(from),
                kind: GroupCommandKind::Release {
                    moves: vec![(id, to)],
                },
            });
        }
    }

    /// Run one tick of the whole world: every group simulates it, then everything they sent
    /// is delivered (actions, handoffs, spawns, reports), and the cells answer questions and
    /// build players' views.
    pub fn step(&mut self) {
        self.tick += 1;
        let tick = self.tick;
        // The gateway sends again the commands and inputs nobody confirmed in time.
        let copies = self.sure.due(tick).resend;
        self.pending
            .extend(copies.into_iter().map(|(s, env)| (s, Msg::Envelope(env))));
        for (subject, msg) in std::mem::take(&mut self.pending) {
            self.deliver(&subject, msg);
        }

        let mut reports = Vec::new();
        let mut sent: Vec<(String, Msg)> = Vec::new();
        let mut spawned = Vec::new();
        for (&id, g) in self.groups.iter_mut() {
            g.step(tick);
            reports.extend(g.take_reports(tick));
            sent.extend(g.take_releases(tick).into_iter().map(|(to, env)| {
                (
                    worldmesh_core::subjects::group_inbox_subject(to),
                    Msg::Envelope(env),
                )
            }));
            sent.extend(
                g.take_outbox()
                    .into_iter()
                    .map(|(s, env)| (s, Msg::Envelope(env))),
            );
            sent.extend(g.take_acks().into_iter().map(|(s, a)| (s, Msg::Acks(a))));
            spawned.extend(g.take_spawns().into_iter().map(|e| (id, e)));
        }

        self.events.clear();
        for (cell, report) in reports {
            for ev in &report.events {
                self.events.push(WireEvent {
                    msg: ev.msg,
                    body: ev.body.clone(),
                });
            }
            if let Some(c) = self.cells.get_mut(&cell) {
                c.receive_report(report);
            }
        }
        // Spawned entities stay in the group that spawned them (the orchestrator would pick
        // one by where they stand).
        for (group, mut e) in spawned {
            e.id = self.next_spawn_id;
            self.next_spawn_id += 1;
            e.player = false;
            self.place(group, e);
        }

        // Where players are looking: at their own entity, as the gateway does. One gateway
        // holds every player here.
        let viewers: Vec<Viewer> = self
            .viewers
            .iter()
            .filter_map(|&p| {
                self.entity(p).map(|e| Viewer {
                    player: p,
                    x: e.x,
                    y: e.y,
                })
            })
            .collect();
        self.views.clear();
        let mut answers = Vec::new();
        for c in self.cells.values_mut() {
            c.receive_viewers(Viewers {
                gateway: GATEWAY,
                tick,
                viewers: viewers.clone(),
            });
            answers.extend(c.answer(tick));
            self.views
                .extend(c.build_views(tick).into_iter().map(|(_, views)| views));
            // Area actions it passes on again, and its confirmations.
            c.resend(tick);
            sent.extend(
                c.take_outbox()
                    .into_iter()
                    .map(|(s, env)| (s, Msg::Envelope(env))),
            );
            sent.extend(c.take_acks().into_iter().map(|(s, a)| (s, Msg::Acks(a))));
        }
        for (group, a) in answers {
            if let Some(g) = self.groups.get_mut(&group) {
                g.receive_senses(a);
            }
        }
        // Delivered before the next tick, like the bus does. Area actions go through the
        // cells right away (they don't wait for a tick either).
        self.pending = sent;
    }

    /// Run `n` ticks.
    pub fn run(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Run ticks until `done` holds (true), or give up after `max` ticks (false).
    pub fn run_until(&mut self, max: u64, mut done: impl FnMut(&Self) -> bool) -> bool {
        for _ in 0..max {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    /// Run for about `ms` milliseconds of world time.
    pub fn run_ms(&mut self, ms: u64) {
        self.run(time::ticks_in(ms));
    }

    /// Should the next message be dropped ([`TestWorld::lose`])?
    fn lost(&mut self) -> bool {
        if self.loss == 0 {
            return false;
        }
        self.draws += 1;
        let lost = codec::splitmix64(self.draws ^ 0x5eed) % 100 < u64::from(self.loss);
        self.dropped += u64::from(lost);
        lost
    }

    fn deliver(&mut self, subject: &str, msg: Msg) {
        if self.lost() {
            return;
        }
        let env = match msg {
            Msg::Envelope(env) => env,
            Msg::Acks(acks) => return self.deliver_acks(subject, acks),
        };
        let mut parts = subject.split('.');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("entity"), Some(id), None) => {
                let Ok(id) = id.parse::<EntityId>() else {
                    return;
                };
                // Everyone listening hears it: the entity's group, and its previous group for
                // a moment after a handoff (which forwards it; receivers drop duplicates).
                for g in self.groups.values_mut() {
                    if g.listening_entities().contains(&id) {
                        g.receive(env.clone());
                    }
                }
            }
            _ if subject == BROADCAST => {
                for g in self.groups.values_mut() {
                    g.receive(env.clone());
                }
            }
            (Some("group"), Some(id), Some("inbox")) => {
                if let Some(g) = id
                    .parse()
                    .ok()
                    .and_then(|id: GroupId| self.groups.get_mut(&id))
                {
                    g.receive(env);
                }
            }
            (Some("spatial"), Some(id), Some("inbox")) => {
                let Some(c) = id
                    .parse()
                    .ok()
                    .and_then(|id: GroupId| self.cells.get_mut(&id))
                else {
                    return;
                };
                c.route_area(env, self.tick);
                for (subject, env) in c.take_outbox() {
                    self.deliver(&subject, Msg::Envelope(env));
                }
            }
            _ => {}
        }
    }

    fn deliver_acks(&mut self, subject: &str, acks: Acks) {
        let mut parts = subject.split('.');
        let (Some(role), Some(id)) = (parts.next(), parts.next().and_then(|p| p.parse().ok()))
        else {
            return;
        };
        match role {
            "group" => {
                if let Some(g) = self.groups.get_mut(&id) {
                    g.receive_acks(acks);
                }
            }
            "spatial" => {
                if let Some(c) = self.cells.get_mut(&id) {
                    c.receive_acks(acks);
                }
            }
            "gateway" if id == GATEWAY => {
                for key in acks.envelopes {
                    self.sure.confirm(acks.by, key);
                }
            }
            _ => {}
        }
    }

    /// An entity, wherever it is.
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.groups.values().find_map(|g| g.entity(id))
    }

    /// The group simulating an entity.
    pub fn group_of(&self, id: EntityId) -> Option<GroupId> {
        self.groups
            .iter()
            .find(|(_, g)| g.entity(id).is_some())
            .map(|(&id, _)| id)
    }

    /// Every entity in the world: (group, entity).
    pub fn entities(&self) -> Vec<(GroupId, &Entity)> {
        self.groups
            .iter()
            .flat_map(|(&id, g)| g.entities().map(move |e| (id, e)))
            .collect()
    }

    /// The events of type `E` from the last step, in the order they happened.
    pub fn events<E: Message>(&self) -> Vec<E> {
        decode_events(&self.rules, &self.events)
    }

    /// The names of every event from the last step, in the order they happened.
    pub fn event_names(&self) -> Vec<&'static str> {
        self.events
            .iter()
            .filter_map(|e| self.rules.message(e.msg).map(|m| m.name))
            .collect()
    }

    /// What `player` could see after the last step (every cell's part, merged).
    pub fn view(&self, player: EntityId) -> Vec<Seen> {
        let cells: BTreeSet<u32> = self.views.iter().map(|v| v.spatial).collect();
        let (entities, _, _) = merge_view(player, &self.views, &cells);
        entities
            .into_iter()
            .map(|e| Seen {
                id: e.id,
                x: e.x,
                y: e.y,
                group: e.group,
                spatial: e.spatial,
                player: e.player,
                kind: self.rules.kind_name(e.kind).to_string(),
                components: e
                    .parts
                    .iter()
                    .filter_map(|(c, b)| {
                        Some((self.rules.component(*c)?.name.to_string(), b.to_vec()))
                    })
                    .collect(),
            })
            .collect()
    }

    /// The events of type `E` that `player` got with their last view.
    pub fn events_seen<E: Message>(&self, player: EntityId) -> Vec<E> {
        let cells: BTreeSet<u32> = self.views.iter().map(|v| v.spatial).collect();
        let (_, events, _) = merge_view(player, &self.views, &cells);
        decode_events(&self.rules, &events)
    }
}

fn decode_events<E: Message>(rules: &Rules, events: &[WireEvent]) -> Vec<E> {
    let Some(id) = rules.message_id::<E>() else {
        return Vec::new();
    };
    events
        .iter()
        .filter(|e| e.msg == id)
        .filter_map(|e| codec::from_bytes(&e.body))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use worldmesh_sim::test_game::{self, Command, Health, Hurt, Input, Log};

    fn hp(w: &TestWorld, id: EntityId) -> i32 {
        w.entity(id).unwrap().get::<Health>().unwrap().0
    }

    /// Alice in group 1 and Bob in group 2, both at (10.5, 10.5).
    fn two_players() -> (TestWorld, EntityId, EntityId) {
        let mut w = TestWorld::new(test_game::world(), 2, 2);
        let alice = w.join(1);
        let bob = w.join_in(2, 2);
        w.step();
        (w, alice, bob)
    }

    #[test]
    fn an_action_reaches_an_entity_in_another_group_on_the_next_tick() {
        let (mut w, alice, bob) = two_players();
        w.command(alice, Command::Hit { target: bob });
        w.step();
        assert_eq!(hp(&w, bob), 100, "sent this tick");
        w.step();
        let tick = w.tick();
        assert_eq!(
            w.entity(bob).unwrap().get::<Log>().unwrap().0,
            vec![format!("dmg 10 from {alice} on {tick}")]
        );
        assert_eq!(w.events::<Hurt>(), vec![Hurt { id: bob, hp: 90 }]);
        assert_eq!(w.event_names(), vec!["hurt"]);
        assert_eq!(
            w.events_seen::<Hurt>(alice),
            vec![Hurt { id: bob, hp: 90 }],
            "Alice can see where it happened"
        );
    }

    #[test]
    fn area_actions_go_through_the_cells_and_everyone_means_everyone() {
        let (mut w, alice, bob) = two_players();
        w.command(
            alice,
            Command::Blast {
                x: 10.5,
                y: 10.5,
                r: 2.0,
            },
        );
        w.run(2);
        assert_eq!(hp(&w, bob), 95);
        assert_eq!(hp(&w, alice), 100, "not the sender");
        w.command(bob, Command::Shout);
        w.run(2);
        assert!(w
            .entity(alice)
            .unwrap()
            .get::<Log>()
            .unwrap()
            .0
            .last()
            .unwrap()
            .ends_with(" all"));
    }

    #[test]
    fn a_handed_off_entity_keeps_getting_its_actions() {
        let (mut w, alice, bob) = two_players();
        w.hand_off(bob, 1);
        w.command(alice, Command::Hit { target: bob });
        w.run(3);
        assert_eq!(w.group_of(bob), Some(1));
        assert_eq!(hp(&w, bob), 90, "exactly once");
    }

    #[test]
    fn spawned_entities_get_ids_and_start_ticking() {
        let (mut w, alice, _) = two_players();
        w.command(alice, Command::Spawn);
        w.run(2);
        let spawned: Vec<_> = w
            .entities()
            .into_iter()
            .filter(|(_, e)| e.id >= SPAWNED_ID_START)
            .collect();
        assert_eq!(spawned.len(), 1);
        let (group, e) = spawned[0];
        assert_eq!(
            (group, e.get::<Health>(), e.player),
            (1, Some(&Health(7)), false)
        );
    }

    #[test]
    fn inputs_take_effect_at_their_sub_step_and_views_show_visible_components() {
        let (mut w, alice, bob) = two_players();
        w.input(alice, 8, Input { dx: 1.0, dy: 0.0 });
        w.step();
        assert_eq!(
            w.entity(alice).unwrap().x,
            11.0,
            "half a tile: from sub-step 8 of 16"
        );
        let seen: Vec<(EntityId, GroupId, Option<Health>, bool)> = w
            .view(alice)
            .into_iter()
            .map(|s| (s.id, s.group, s.get::<Health>(), s.has::<Log>()))
            .collect();
        assert_eq!(
            seen,
            vec![
                (alice, 1, Some(Health(100)), false),
                (bob, 2, Some(Health(100)), false)
            ]
        );
        w.leave(bob);
        w.step();
        assert!(w.entity(bob).is_none());
        assert_eq!(w.view(alice).len(), 1);
    }

    fn lines(w: &TestWorld, id: EntityId, prefix: &str) -> usize {
        w.entity(id)
            .unwrap()
            .get::<Log>()
            .unwrap()
            .0
            .iter()
            .filter(|l| l.starts_with(prefix))
            .count()
    }

    #[test]
    fn on_a_lossy_bus_sure_actions_land_exactly_once_and_throwaway_ones_sometimes_dont() {
        let (mut w, alice, bob) = two_players();
        w.lose(30);
        for _ in 0..40 {
            w.command(alice, Command::Hit { target: bob }); // sure command, sure action
            w.command(alice, Command::Poke { target: bob }); // sure command, throwaway action
            w.step();
        }
        for _ in 0..5 {
            w.command(
                alice,
                Command::Blast {
                    x: 10.5,
                    y: 10.5,
                    r: 2.0,
                },
            );
            w.command(alice, Command::Shout);
            w.step();
        }
        assert!(
            w.run_until(200, |w| w.unconfirmed() == 0),
            "everything confirmed"
        );
        assert!(w.dropped() > 100, "the bus really was lossy");
        assert_eq!(lines(&w, bob, "dmg 10 "), 40, "every hit, once");
        assert_eq!(lines(&w, bob, "dmg 5 "), 5, "every blast, once");
        assert_eq!(
            w.entity(bob)
                .unwrap()
                .get::<Log>()
                .unwrap()
                .0
                .iter()
                .filter(|l| l.ends_with(" all"))
                .count(),
            5,
            "every shout, once"
        );
        let pokes = lines(&w, bob, "dmg 1 ") - 5; // shouts do 1 damage too
        assert!(
            (10..40).contains(&pokes),
            "about 30% of pokes lost: {pokes} of 40 landed"
        );
    }

    #[test]
    fn on_a_lossy_bus_a_player_who_stops_stops() {
        let (mut w, alice, _) = two_players();
        w.lose(30);
        for round in 0..40 {
            let dx = if round % 2 == 0 { 1.0 } else { -1.0 };
            w.input(alice, 0, Input { dx, dy: 0.0 });
            w.run(3);
            w.input(alice, 0, Input { dx: 0.0, dy: 0.0 });
            w.run(30); // long enough for the stop to be sent again, if it was dropped
            let x = w.entity(alice).unwrap().x;
            w.run(10);
            assert_eq!(
                w.entity(alice).unwrap().x,
                x,
                "still walking after round {round}"
            );
        }
        assert!(
            w.dropped() > 5,
            "the bus really was lossy: {} dropped",
            w.dropped()
        );
        assert!(w.run_until(100, |w| w.unconfirmed() == 0));
    }

    #[test]
    fn on_a_lossy_bus_handoffs_never_lose_or_copy_an_entity() {
        let (mut w, alice, bob) = two_players();
        w.lose(30);
        // Bob changes groups every second (far more often than scaling would move anyone),
        // and is hit every 2 ticks.
        for i in 0..20 {
            w.hand_off(bob, if i % 2 == 0 { 1 } else { 2 });
            for _ in 0..5 {
                w.command(alice, Command::Hit { target: bob });
                w.run(2);
            }
        }
        assert!(w.run_until(200, |w| w.unconfirmed() == 0));
        let copies = w.entities().iter().filter(|(_, e)| e.id == bob).count();
        assert_eq!(copies, 1, "bob exists exactly once");
        assert_eq!(lines(&w, bob, "dmg 10 "), 100, "and got every hit once");
    }

    #[test]
    fn entities_can_be_placed_directly() {
        let mut w = TestWorld::new(test_game::world(), 1, 1);
        let id = w.spawn_in(1, Spawn::new("unit", 5.5, 5.5).with(Health(3)));
        w.step();
        assert_eq!(
            (hp(&w, id), w.entity(id).unwrap().x),
            (3, 5.5),
            "the kind's defaults, with its own health"
        );
    }
}
