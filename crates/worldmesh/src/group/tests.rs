use super::*;
use crate::config::ViewShape;
use crate::game::{Spawn, To};
use crate::protocol::{SenseAnswer, Seen};
use serde::{Deserialize, Serialize};

// ---- A tiny game: entities walk at one tile per tick and take damage ----

pub(crate) struct TestGame;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Data {
    pub hp: i32,
    pub dir: (f32, f32),
    /// What happened to it: "dmg <n> from <sender> on <tick>[ fwd][ area][ all]".
    pub log: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct View {
    pub hp: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Input {
    pub dx: f32,
    pub dy: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum Command {
    Hit { target: u32 },
    Blast { x: f32, y: f32, r: f32 },
    Shout,
    Spawn,
    Vanish,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum Action {
    Damage(i32),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "e", rename_all = "snake_case")]
pub(crate) enum Event {
    Hurt { id: u32, hp: i32 },
}

const SUB: f32 = 16.0;

impl Game for TestGame {
    type Data = Data;
    type View = View;
    type Input = Input;
    type Command = Command;
    type Action = Action;
    type Event = Event;
    type Welcome = ();

    fn config(&self) -> WorldConfig {
        WorldConfig { width: 64, height: 64, chunk_size: 8, view: ViewShape::Rect { half_w: 20.0, half_h: 12.0 }, input_substeps: 16 }
    }

    fn welcome(&self) {}

    fn join(&self, _player: EntityId) -> Spawn<Data> {
        Spawn { x: 10.5, y: 10.5, data: Data { hp: 100, ..Data::default() } }
    }

    fn view(&self, e: &Entity<Data>) -> View {
        View { hp: e.data.hp }
    }

    /// One tile per tick in the held direction, changing direction at each input's sub-step.
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Input>]) {
        let mut from = 0.0;
        for t in inputs {
            let at = f32::from(t.sub);
            let (dx, dy) = me.data.dir;
            me.x += dx * (at - from) / SUB;
            me.y += dy * (at - from) / SUB;
            from = at;
            me.data.dir = (t.input.dx, t.input.dy);
        }
        let (dx, dy) = me.data.dir;
        me.x += dx * (SUB - from) / SUB;
        me.y += dy * (SUB - from) / SUB;
    }

    fn on_command(&self, me: &mut Me<'_, Self>, command: Command) {
        match command {
            Command::Hit { target } => me.send(To::Entity(target), Action::Damage(10)),
            Command::Blast { x, y, r } => me.send(To::Area { x, y, r }, Action::Damage(5)),
            Command::Shout => me.send(To::Everyone, Action::Damage(1)),
            Command::Spawn => {
                let (x, y) = (me.x + 1.0, me.y);
                me.spawn(x, y, Data { hp: 7, ..Data::default() });
            }
            Command::Vanish => me.despawn(),
        }
    }

    fn on_action(&self, me: &mut Me<'_, Self>, action: Action, from: &Incoming) {
        let Action::Damage(n) = action;
        me.data.hp -= n;
        let mut line = format!("dmg {n} from {} on {}", from.from.id, me.tick());
        if from.forwarded {
            line += " fwd";
        }
        if from.area.is_some() {
            line += " area";
        }
        if from.everyone {
            line += " all";
        }
        me.data.log.push(line);
        let (id, hp) = (me.id, me.data.hp);
        me.emit(Event::Hurt { id, hp });
    }
}

// ---- Helpers ----

const WEST: u32 = 0x8001;
const EAST: u32 = 0x8002;

/// Spatial Layer Cell WEST owns chunk x < 4 (tiles x < 32), EAST the rest.
fn west_east() -> Assignment {
    let owners = (0..64u32).map(|i| if i % 8 < 4 { WEST } else { EAST }).collect();
    Assignment {
        version: 1,
        chunk_size: 8,
        chunks_x: 8,
        chunks_y: 8,
        owners,
        spatial: vec![WEST, EAST],
        groups: vec![1, 2],
        spatial_running: 2,
        groups_running: 2,
        group_hosts: 1,
        binary_bus: false,
        group_lag: Vec::new(),
    }
}

fn world(id: u32) -> GroupWorld<TestGame> {
    let mut w = GroupWorld::new(id, Arc::new(TestGame));
    w.set_assignment(&west_east());
    w
}

fn spawn_at(w: &mut GroupWorld<TestGame>, id: u32, x: f32, y: f32, player: bool) {
    let data = codec::to_bytes(&Data { hp: 100, ..Data::default() }).unwrap();
    let n = NewEntity { id, x, y, player, data };
    w.command(GroupCommand { to: None, kind: GroupCommandKind::Spawn { entities: vec![n] } });
}

fn x(w: &GroupWorld<TestGame>, id: u32) -> f32 {
    w.entity(id).unwrap().x
}

fn data(w: &GroupWorld<TestGame>, id: u32) -> &Data {
    &w.entity(id).unwrap().data
}

fn envelope(to: Address, from: u32, seq: u64, kind: Kind) -> Envelope {
    Envelope { to, from: Facts { id: from, ..Facts::default() }, via: 0, seq, trace: 7, hops: 0, kind }
}

/// What the gateway sends for a player's input.
fn input(id: u32, seq: u64, dx: f32, dy: f32, at_tick: u64, sub: u8) -> Envelope {
    let input = codec::to_bytes(&Input { dx, dy }).unwrap();
    envelope(Address::Entity { id }, id, seq, Kind::Input { at_tick, sub, input })
}

/// What the gateway sends for a player's command (stamped at `tick`).
fn command(player: u32, seq: u64, cmd: Command, tick: u64) -> Envelope {
    let mut env = envelope(Address::Entity { id: player }, player, seq, Kind::Command { command: codec::to_bytes(&cmd).unwrap() });
    env.from.tick = tick;
    env
}

fn release(w: &mut GroupWorld<TestGame>, id: u32, to: u32) {
    w.command(GroupCommand { to: Some(w.group_id), kind: GroupCommandKind::Release { moves: vec![(id, to)] } });
}

/// Deliver everything `from` published to whichever of `groups` should get it.
fn deliver(from: &mut GroupWorld<TestGame>, groups: &mut [&mut GroupWorld<TestGame>]) -> Vec<String> {
    let out = from.take_outbox();
    let subjects = out.iter().map(|(s, _)| s.clone()).collect();
    for (subject, env) in out {
        for w in groups.iter_mut() {
            let mine = subject == group_inbox_subject(w.group_id)
                || subject == BROADCAST
                || matches!(env.to, Address::Entity { id } if subject == entity_subject(id) && w.listening_entities().contains(&id));
            if mine {
                w.receive(env.clone());
            }
        }
    }
    subjects
}

fn hand_off(a: &mut GroupWorld<TestGame>, b: &mut GroupWorld<TestGame>, id: u32, tick: u64) {
    release(a, id, b.group_id);
    for (_, env) in a.take_releases(tick) {
        b.receive(env);
    }
}

// ---- Inputs ----

#[test]
fn input_takes_effect_exactly_at_its_tick() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    w.receive(input(1, 1, 1.0, 0.0, 1005, 0));
    for t in 1001..=1004 {
        w.step(t);
    }
    assert_eq!(x(&w, 1), 10.0, "moved before its tick");
    w.step(1005);
    assert_eq!(x(&w, 1), 11.0);
}

#[test]
fn late_input_takes_effect_at_the_start_of_the_next_tick() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    w.step(1000);
    w.receive(input(1, 1, 1.0, 0.0, 990, 8));
    w.step(1001);
    assert_eq!(x(&w, 1), 11.0, "a whole tick of movement: its sub-step no longer applies");
}

#[test]
fn inputs_change_direction_at_their_sub_step() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    w.receive(input(1, 1, 1.0, 0.0, 1001, 4));
    w.receive(input(1, 2, 0.0, 0.0, 1001, 12));
    w.step(1001);
    assert_eq!(x(&w, 1), 10.5, "walked from sub-step 4 to 12 of 16");
}

#[test]
fn of_two_inputs_for_the_same_moment_the_later_sent_wins() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    w.receive(input(1, 2, 1.0, 0.0, 1001, 0)); // newer
    w.receive(input(1, 1, -1.0, 0.0, 1001, 0)); // older, arriving late (e.g. forwarded)
    w.step(1001);
    assert_eq!(x(&w, 1), 11.0);
}

#[test]
fn input_the_game_cant_read_is_dropped() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    let bad = envelope(Address::Entity { id: 1 }, 1, 1, Kind::Input { at_tick: 1001, sub: 0, input: vec![0xc1] });
    w.receive(bad);
    w.step(1001);
    assert_eq!(x(&w, 1), 10.0);
}

// ---- Handoffs ----

#[test]
fn a_released_entity_keeps_its_data_and_pending_inputs_and_is_never_simulated_twice() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 1, 10.0, 10.0, true);
    a.receive(input(1, 1, 1.0, 0.0, 0, 0));
    a.receive(input(1, 2, 0.0, 1.0, 1003, 0)); // still pending at the handoff
    a.step(1001);
    hand_off(&mut a, &mut b, 1, 1001);
    assert!(a.entity(1).is_none());
    assert_eq!(x(&b, 1), 11.0);

    b.step(1001); // the tick the old group already simulated
    assert_eq!(x(&b, 1), 11.0, "simulated twice for one tick");
    b.step(1002);
    assert_eq!(x(&b, 1), 12.0, "keeps walking east (its data came along)");
    b.step(1003);
    assert_eq!((x(&b, 1), b.entity(1).unwrap().y), (12.0, 11.0), "the pending input came along");
}

#[test]
fn inputs_that_reach_the_old_group_after_a_handoff_are_forwarded() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 1, 10.0, 10.0, true);
    hand_off(&mut a, &mut b, 1, 1000);
    assert!(a.listening_entities().contains(&1), "still listening, to forward strays");
    a.receive(input(1, 5, -1.0, 0.0, 1002, 0));
    assert_eq!(deliver(&mut a, &mut [&mut b]), vec![group_inbox_subject(2)]);
    b.step(1002);
    assert_eq!(x(&b, 1), 9.0);
}

#[test]
fn a_late_handoff_for_a_departed_player_is_dropped() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 7, 10.0, 10.0, true);
    b.command(GroupCommand { to: None, kind: GroupCommandKind::Leave { id: 7 } });
    hand_off(&mut a, &mut b, 7, 1000);
    assert_eq!(b.entity_count(), 0);
}

#[test]
fn commands_for_another_group_are_ignored() {
    let mut w = world(1);
    let data = codec::to_bytes(&Data::default()).unwrap();
    let n = NewEntity { id: 3, x: 30.5, y: 30.5, player: false, data };
    w.command(GroupCommand { to: Some(2), kind: GroupCommandKind::Spawn { entities: vec![n] } });
    assert_eq!(w.entity_count(), 0);
}

// ---- Commands and actions ----

#[test]
fn an_action_for_an_entity_in_another_group_goes_through_the_bus_and_lands_next_tick() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 1, 10.0, 10.0, true);
    spawn_at(&mut b, 99, 12.0, 10.0, false);
    a.receive(command(1, 1, Command::Hit { target: 99 }, 1000));
    a.step(1001); // the player's group handles the command: the action goes out
    assert_eq!(deliver(&mut a, &mut [&mut b]), vec![entity_subject(99)]);
    b.step(1001);
    assert!(data(&b, 99).log.is_empty(), "sent on tick 1001: lands on 1002, not before");
    b.step(1002);
    assert_eq!(data(&b, 99).log, vec!["dmg 10 from 1 on 1002"]);
    assert_eq!(b.counters.actions_received, 1);
}

#[test]
fn actions_land_on_the_next_tick_even_in_the_same_group() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    spawn_at(&mut w, 2, 11.0, 10.0, false);
    w.receive(command(1, 1, Command::Hit { target: 2 }, 1000));
    w.step(1001);
    assert!(w.take_outbox().is_empty(), "stays in the group");
    assert!(data(&w, 2).log.is_empty());
    w.step(1002);
    assert_eq!(data(&w, 2).log, vec!["dmg 10 from 1 on 1002"]);
    assert_eq!(w.counters.actions_local, 1);
}

#[test]
fn an_action_that_arrives_early_still_waits_for_the_next_tick() {
    let mut w = world(2);
    spawn_at(&mut w, 99, 12.0, 10.0, false);
    // Group 1 is a little ahead and already ran tick 1001; we haven't yet.
    let mut env = envelope(Address::Entity { id: 99 }, 1, 1, Kind::Action { action: codec::to_bytes(&Action::Damage(3)).unwrap() });
    env.from.tick = 1001;
    w.receive(env);
    w.step(1001);
    assert!(data(&w, 99).log.is_empty());
    w.step(1002);
    assert_eq!(data(&w, 99).log, vec!["dmg 3 from 1 on 1002"]);
}

#[test]
fn a_player_can_only_command_their_own_entity() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    spawn_at(&mut w, 2, 11.0, 10.0, true);
    // Player 2's command, addressed to player 1's entity.
    let mut env = command(2, 1, Command::Hit { target: 2 }, 1000);
    env.to = Address::Entity { id: 1 };
    w.receive(env);
    w.step(1001);
    w.step(1002);
    assert!(data(&w, 2).log.is_empty());
}

#[test]
fn an_action_for_an_entity_that_just_left_is_forwarded_and_applied_once() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 99, 12.0, 10.0, false);
    hand_off(&mut a, &mut b, 99, 1000);
    // Group 3's action reaches both: the old group (stale subscription) and the new one.
    let mut env = envelope(Address::Entity { id: 99 }, 5, 1, Kind::Action { action: codec::to_bytes(&Action::Damage(4)).unwrap() });
    env.from = Facts { id: 5, group: 3, x: 0.0, y: 0.0, tick: 1000 };
    a.receive(env.clone());
    b.receive(env);
    a.step(1001);
    assert_eq!(deliver(&mut a, &mut [&mut b]), vec![group_inbox_subject(2)]);
    b.step(1001);
    b.step(1002);
    assert_eq!(data(&b, 99).log, vec!["dmg 4 from 5 on 1001"], "applied once, not twice");
}

#[test]
fn a_command_that_misses_its_player_mid_handoff_is_forwarded() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 1, 10.0, 10.0, true);
    spawn_at(&mut b, 2, 11.0, 10.0, false);
    hand_off(&mut a, &mut b, 1, 1000);
    a.receive(command(1, 1, Command::Hit { target: 2 }, 1000));
    a.step(1001);
    deliver(&mut a, &mut [&mut b]);
    b.step(1001); // the forwarded command is handled; its action goes to entity 2, here
    b.step(1002);
    assert_eq!(data(&b, 2).log, vec!["dmg 10 from 1 on 1002"]);
}

#[test]
fn an_area_action_goes_to_every_cell_its_circle_touches() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 30.0, 10.0, true);
    w.receive(command(1, 1, Command::Blast { x: 31.0, y: 10.0, r: 3.0 }, 1000));
    w.step(1001);
    let mut subjects: Vec<String> = w.take_outbox().into_iter().map(|(s, _)| s).collect();
    subjects.sort();
    assert_eq!(subjects, vec![spatial_inbox_subject(WEST), spatial_inbox_subject(EAST)]);
}

#[test]
fn an_area_action_lands_once_on_our_entities_in_the_circle_but_not_the_sender() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 20.0, 20.0, true);
    spawn_at(&mut w, 2, 21.0, 20.0, false);
    spawn_at(&mut w, 3, 30.0, 20.0, false);
    let mut env = envelope(Address::Area { x: 20.0, y: 20.0, r: 2.0 }, 1, 1, Kind::Action { action: codec::to_bytes(&Action::Damage(5)).unwrap() });
    env.from.tick = 1000;
    w.receive(env.clone());
    w.receive(env); // a second cell passes on the same action
    w.step(1001);
    assert!(data(&w, 1).log.is_empty(), "not the sender");
    assert_eq!(data(&w, 2).log, vec!["dmg 5 from 1 on 1001 area"]);
    assert!(data(&w, 3).log.is_empty(), "outside the circle");
}

#[test]
fn an_action_for_everyone_reaches_every_entity_once() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 1, 10.0, 10.0, true);
    spawn_at(&mut b, 2, 40.0, 10.0, false);
    a.receive(command(1, 1, Command::Shout, 1000));
    a.step(1001);
    let out = a.take_outbox();
    assert_eq!(out.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), vec![BROADCAST]);
    for (_, env) in out {
        a.receive(env.clone());
        b.receive(env.clone());
        b.receive(env); // delivered twice: still once
    }
    a.step(1002);
    b.step(1002);
    assert_eq!(data(&a, 1).log, vec!["dmg 1 from 1 on 1002 all"]);
    assert_eq!(data(&b, 2).log, vec!["dmg 1 from 1 on 1002 all"]);
}

#[test]
fn spawns_go_to_the_orchestrator_and_despawned_entities_are_gone() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    spawn_at(&mut w, SPAWNED, 12.0, 10.0, false);
    w.receive(command(1, 1, Command::Spawn, 1000));
    w.receive(command(SPAWNED, 1, Command::Vanish, 1000));
    w.step(1001);
    let spawns = w.take_spawns();
    assert_eq!(spawns.len(), 1);
    assert_eq!((spawns[0].x, spawns[0].player), (11.0, false));
    assert_eq!(codec::from_bytes::<Data>(&spawns[0].data).unwrap().hp, 7);
    assert!(w.entity(SPAWNED).is_none());
}

const SPAWNED: u32 = crate::protocol::SPAWNED_ID_START + 5;

// ---- The spatial layer ----

#[test]
fn each_entity_is_reported_to_the_cell_of_where_it_stands_with_its_view() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    spawn_at(&mut w, 2, 40.0, 10.0, false);
    w.step(1001);
    let reports: BTreeMap<u32, GroupReport> = w.take_reports(1001).into_iter().collect();
    assert_eq!(reports[&WEST].entities.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1]);
    assert_eq!(reports[&EAST].entities.iter().map(|e| e.id).collect::<Vec<_>>(), vec![2]);
    let view: View = codec::from_bytes(&reports[&WEST].entities[0].view).unwrap();
    assert_eq!(view, View { hp: 100 });
    assert!(reports[&WEST].entities[0].player);
}

#[test]
fn events_go_to_the_cell_of_where_they_happened() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    spawn_at(&mut w, 2, 40.0, 10.0, false);
    let mut env = envelope(Address::Entity { id: 2 }, 1, 1, Kind::Action { action: codec::to_bytes(&Action::Damage(5)).unwrap() });
    env.from.tick = 1000;
    w.receive(env);
    w.step(1001);
    let reports: BTreeMap<u32, GroupReport> = w.take_reports(1001).into_iter().collect();
    assert!(reports[&WEST].events.is_empty());
    let ev: Event = codec::from_bytes(&reports[&EAST].events[0].event).unwrap();
    assert_eq!(ev, Event::Hurt { id: 2, hp: 95 });
}

/// A game that asks what's near and remembers what it found.
struct Asker;
impl Game for Asker {
    type Data = Vec<(u32, bool)>;
    type View = View;
    type Input = Input;
    type Command = Command;
    type Action = Action;
    type Event = Event;
    type Welcome = ();
    fn config(&self) -> WorldConfig {
        TestGame.config()
    }
    fn welcome(&self) {}
    fn join(&self, _: EntityId) -> Spawn<Self::Data> {
        Spawn { x: 0.0, y: 0.0, data: Vec::new() }
    }
    fn view(&self, _: &Entity<Self::Data>) -> View {
        View { hp: 1 }
    }
    fn tick(&self, me: &mut Me<'_, Self>, _: &[Timed<Input>]) {
        me.query(6.0);
        let found: Vec<(u32, bool)> = me.nearby(6.0).into_iter().map(|n| (n.id, n.local)).collect();
        me.data = found;
    }
}

#[test]
fn questions_go_with_the_reports_and_answers_show_up_in_nearby() {
    let mut w = GroupWorld::new(1, Arc::new(Asker));
    w.set_assignment(&west_east());
    let data = codec::to_bytes(&Vec::<(u32, bool)>::new()).unwrap();
    for (id, x) in [(1, 10.0), (2, 12.0)] {
        let n = NewEntity { id, x, y: 10.0, player: false, data: data.clone() };
        w.command(GroupCommand { to: None, kind: GroupCommandKind::Spawn { entities: vec![n] } });
    }
    w.step(1001);
    let reports: BTreeMap<u32, GroupReport> = w.take_reports(1001).into_iter().collect();
    assert_eq!(reports[&WEST].queries.len(), 2);
    // The cell answers: entity 9 of group 2 stands next to entity 1.
    let view = codec::to_bytes(&View { hp: 50 }).unwrap();
    let seen = Seen { id: 9, x: 11.0, y: 11.0, group: 2, player: true, view };
    w.receive_senses(SenseAnswers { spatial: WEST, tick: 1001, answers: vec![SenseAnswer { id: 1, seen: vec![seen] }] });
    w.step(1002);
    let found = &w.entity(1).unwrap().data;
    assert_eq!(found, &vec![(2, true), (9, false)], "its own group's entity exactly, the other group's from the answer");
}

// ---- Fake lag ----

#[test]
fn lag_queue_hands_things_over_in_arrival_order_once_they_have_waited() {
    let mut q = LagQueue::default();
    let t0 = Instant::now();
    let ms = |n: u64| Duration::from_millis(n);
    let cmd = |id: u32| Inbound::Command(GroupCommand { to: None, kind: GroupCommandKind::Leave { id } });
    q.push(t0 + ms(30), cmd(3));
    q.push(t0 + ms(10), cmd(1));
    q.push(t0 + ms(20), cmd(2));
    let ids = |items: Vec<Inbound>| -> Vec<u32> {
        items
            .into_iter()
            .map(|i| match i {
                Inbound::Command(GroupCommand { kind: GroupCommandKind::Leave { id }, .. }) => id,
                _ => 0,
            })
            .collect()
    };
    assert!(q.take_due(t0 + ms(100), ms(100)).is_empty(), "nothing has waited 100 ms yet");
    assert_eq!(ids(q.take_due(t0 + ms(125), ms(100))), vec![1, 2]);
    assert_eq!(ids(q.take_due(t0 + ms(125), ms(100))), Vec::<u32>::new(), "each once");
    assert_eq!(ids(q.take_due(t0 + ms(130), ms(100))), vec![3]);
    q.push(t0 + ms(140), cmd(4));
    assert_eq!(ids(q.take_due(t0 + ms(140), Duration::ZERO)), vec![4]);
}
