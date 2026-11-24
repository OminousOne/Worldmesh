//! Test your game's rules in one process, without NATS, tick by tick.
//!
//! A [`TestWorld`] runs real Server Entity Groups and Spatial Layer Cells and passes their
//! messages between them exactly as the bus would (actions to entities wherever they are,
//! area actions through the cells, handoffs, spatial query results), but synchronously:
//! one [`TestWorld::step`] is one tick of the whole world.
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
    Assignment, EntityId, Envelope, GroupCommand, GroupCommandKind, GroupId, Kind, NewEntity,
    SpatialViews, Viewer, Viewers, WireEvent, SPAWNED_ID_START,
};
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

/// A whole world in one process. See the [module docs](self).
pub struct TestWorld {
    rules: Arc<Rules>,
    config: WorldConfig,
    tick: u64,
    groups: BTreeMap<GroupId, GroupWorld>,
    cells: BTreeMap<GroupId, SpatialWorld>,
    /// Envelopes waiting to be delivered before the next tick: (subject, envelope).
    pending: Vec<(String, Envelope)>,
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
            next_spawn_id: SPAWNED_ID_START,
            events: Vec::new(),
            views: Vec::new(),
            viewers: BTreeSet::new(),
        }
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
        self.pending
            .push((worldmesh_core::subjects::entity_subject(player), env));
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
        self.pending
            .push((worldmesh_core::subjects::entity_subject(player), env));
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
        for (subject, env) in std::mem::take(&mut self.pending) {
            self.deliver(&subject, env);
        }

        let mut reports = Vec::new();
        let mut sent = Vec::new();
        let mut spawned = Vec::new();
        for (&id, g) in self.groups.iter_mut() {
            g.step(tick);
            reports.extend(g.take_reports(tick));
            sent.extend(
                g.take_releases(tick)
                    .into_iter()
                    .map(|(to, env)| (worldmesh_core::subjects::group_inbox_subject(to), env)),
            );
            sent.extend(g.take_outbox());
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

    fn deliver(&mut self, subject: &str, env: Envelope) {
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
                c.route_area(env);
                for (subject, env) in c.take_outbox() {
                    self.deliver(&subject, env);
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
