use super::*;
use crate::test_game::*;
use crate::world::{Stage, World};
use crate::{Bundle, Component, Spawn};
use worldmesh_core::codec;
use worldmesh_core::protocol::{Seen, SenseAnswer};

const WEST: u32 = 0x8001;
const EAST: u32 = 0x8002;

/// Spatial Layer Cell WEST owns chunk x < 4 (tiles x < 32), EAST the rest.
fn west_east() -> Assignment {
    let owners = (0..64u32)
        .map(|i| if i % 8 < 4 { WEST } else { EAST })
        .collect();
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

fn world(id: u32) -> GroupWorld {
    let mut w = GroupWorld::new(id, rules());
    w.set_assignment(&west_east());
    w
}

/// A "unit" (the test game's only kind) at (x, y).
fn spawn_at(w: &mut GroupWorld, id: u32, x: f32, y: f32, player: bool) {
    let n = NewEntity {
        id,
        x,
        y,
        player,
        kind: 0,
        parts: Vec::new(),
    };
    w.command(GroupCommand {
        to: None,
        kind: GroupCommandKind::Spawn { entities: vec![n] },
    });
}

fn x(w: &GroupWorld, id: u32) -> f32 {
    w.entity(id).unwrap().x
}

fn log(w: &GroupWorld, id: u32) -> Vec<String> {
    w.entity(id).unwrap().get::<Log>().unwrap().0.clone()
}

/// The id a message has in the test game.
fn msg<M: crate::Message>() -> u16 {
    rules().message_id::<M>().unwrap()
}

fn damage(n: i32) -> Kind {
    Kind::Action {
        msg: msg::<Damage>(),
        body: codec::to_bytes(&Damage(n)).unwrap(),
    }
}

fn envelope(to: Address, from: u32, seq: u64, kind: Kind) -> Envelope {
    Envelope {
        to,
        from: Facts {
            id: from,
            ..Facts::default()
        },
        via: 0,
        seq,
        trace: 7,
        hops: 0,
        kind,
    }
}

/// What the gateway sends for a player's input.
fn input(id: u32, seq: u64, dx: f32, dy: f32, at_tick: u64, sub: u8) -> Envelope {
    let input = codec::to_bytes(&Input { dx, dy }).unwrap();
    envelope(
        Address::Entity { id },
        id,
        seq,
        Kind::Input {
            at_tick,
            sub,
            input,
        },
    )
}

/// What the gateway sends for a player's command (stamped at `tick`).
fn command(player: u32, seq: u64, cmd: Command, tick: u64) -> Envelope {
    let kind = Kind::Command {
        msg: msg::<Command>(),
        body: codec::to_bytes(&cmd).unwrap(),
    };
    let mut env = envelope(Address::Entity { id: player }, player, seq, kind);
    env.from.tick = tick;
    env
}

fn release(w: &mut GroupWorld, id: u32, to: u32) {
    w.command(GroupCommand {
        to: Some(w.group_id),
        kind: GroupCommandKind::Release {
            moves: vec![(id, to)],
        },
    });
}

/// Deliver everything `from` published to whichever of `groups` should get it.
fn deliver(from: &mut GroupWorld, groups: &mut [&mut GroupWorld]) -> Vec<String> {
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

fn hand_off(a: &mut GroupWorld, b: &mut GroupWorld, id: u32, tick: u64) {
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
    assert_eq!(
        x(&w, 1),
        11.0,
        "a whole tick of movement: its sub-step no longer applies"
    );
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
    let bad = envelope(
        Address::Entity { id: 1 },
        1,
        1,
        Kind::Input {
            at_tick: 1001,
            sub: 0,
            input: vec![0xc1],
        },
    );
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
    assert_eq!(
        (x(&b, 1), b.entity(1).unwrap().y),
        (12.0, 11.0),
        "the pending input came along"
    );
}

#[test]
fn inputs_that_reach_the_old_group_after_a_handoff_are_forwarded() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 1, 10.0, 10.0, true);
    hand_off(&mut a, &mut b, 1, 1000);
    assert!(
        a.listening_entities().contains(&1),
        "still listening, to forward strays"
    );
    a.receive(input(1, 5, -1.0, 0.0, 1002, 0));
    assert_eq!(deliver(&mut a, &mut [&mut b]), vec![group_inbox_subject(2)]);
    b.step(1002);
    assert_eq!(x(&b, 1), 9.0);
}

#[test]
fn a_late_handoff_for_a_departed_player_is_dropped() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 7, 10.0, 10.0, true);
    b.command(GroupCommand {
        to: None,
        kind: GroupCommandKind::Leave { id: 7 },
    });
    hand_off(&mut a, &mut b, 7, 1000);
    assert_eq!(b.entity_count(), 0);
}

#[test]
fn commands_for_another_group_are_ignored() {
    let mut w = world(1);
    let n = NewEntity {
        id: 3,
        x: 30.5,
        y: 30.5,
        player: false,
        kind: 0,
        parts: Vec::new(),
    };
    w.command(GroupCommand {
        to: Some(2),
        kind: GroupCommandKind::Spawn { entities: vec![n] },
    });
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
    assert!(
        log(&b, 99).is_empty(),
        "sent on tick 1001: lands on 1002, not before"
    );
    b.step(1002);
    assert_eq!(log(&b, 99), vec!["dmg 10 from 1 on 1002"]);
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
    assert!(log(&w, 2).is_empty());
    w.step(1002);
    assert_eq!(log(&w, 2), vec!["dmg 10 from 1 on 1002"]);
    assert_eq!(w.counters.actions_local, 1);
}

#[test]
fn an_action_that_arrives_early_still_waits_for_the_next_tick() {
    let mut w = world(2);
    spawn_at(&mut w, 99, 12.0, 10.0, false);
    // Group 1 is a little ahead and already ran tick 1001; we haven't yet.
    let mut env = envelope(Address::Entity { id: 99 }, 1, 1, damage(3));
    env.from.tick = 1001;
    w.receive(env);
    w.step(1001);
    assert!(log(&w, 99).is_empty());
    w.step(1002);
    assert_eq!(log(&w, 99), vec!["dmg 3 from 1 on 1002"]);
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
    assert!(log(&w, 2).is_empty());
}

#[test]
fn an_action_for_an_entity_that_just_left_is_forwarded_and_applied_once() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 99, 12.0, 10.0, false);
    hand_off(&mut a, &mut b, 99, 1000);
    // Group 3's action reaches both: the old group (stale subscription) and the new one.
    let mut env = envelope(Address::Entity { id: 99 }, 5, 1, damage(4));
    env.from = Facts {
        id: 5,
        group: 3,
        x: 0.0,
        y: 0.0,
        tick: 1000,
    };
    a.receive(env.clone());
    b.receive(env);
    a.step(1001);
    assert_eq!(deliver(&mut a, &mut [&mut b]), vec![group_inbox_subject(2)]);
    b.step(1001);
    b.step(1002);
    assert_eq!(
        log(&b, 99),
        vec!["dmg 4 from 5 on 1001"],
        "applied once, not twice"
    );
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
    assert_eq!(log(&b, 2), vec!["dmg 10 from 1 on 1002"]);
}

#[test]
fn an_area_action_goes_to_every_cell_its_circle_touches() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 30.0, 10.0, true);
    w.receive(command(
        1,
        1,
        Command::Blast {
            x: 31.0,
            y: 10.0,
            r: 3.0,
        },
        1000,
    ));
    w.step(1001);
    let mut subjects: Vec<String> = w.take_outbox().into_iter().map(|(s, _)| s).collect();
    subjects.sort();
    assert_eq!(
        subjects,
        vec![spatial_inbox_subject(WEST), spatial_inbox_subject(EAST)]
    );
}

#[test]
fn an_area_action_lands_once_on_our_entities_in_the_circle_but_not_the_sender() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 20.0, 20.0, true);
    spawn_at(&mut w, 2, 21.0, 20.0, false);
    spawn_at(&mut w, 3, 30.0, 20.0, false);
    let mut env = envelope(
        Address::Area {
            x: 20.0,
            y: 20.0,
            r: 2.0,
        },
        1,
        1,
        damage(5),
    );
    env.from.tick = 1000;
    w.receive(env.clone());
    w.receive(env); // a second cell passes on the same action
    w.step(1001);
    assert!(log(&w, 1).is_empty(), "not the sender");
    assert_eq!(log(&w, 2), vec!["dmg 5 from 1 on 1001 area"]);
    assert!(log(&w, 3).is_empty(), "outside the circle");
}

#[test]
fn an_action_for_everyone_reaches_every_entity_once() {
    let (mut a, mut b) = (world(1), world(2));
    spawn_at(&mut a, 1, 10.0, 10.0, true);
    spawn_at(&mut b, 2, 40.0, 10.0, false);
    a.receive(command(1, 1, Command::Shout, 1000));
    a.step(1001);
    let out = a.take_outbox();
    assert_eq!(
        out.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(),
        vec![BROADCAST]
    );
    for (_, env) in out {
        a.receive(env.clone());
        b.receive(env.clone());
        b.receive(env); // delivered twice: still once
    }
    a.step(1002);
    b.step(1002);
    assert_eq!(log(&a, 1), vec!["dmg 1 from 1 on 1002 all"]);
    assert_eq!(log(&b, 2), vec!["dmg 1 from 1 on 1002 all"]);
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
    let health = rules().component_id::<Health>().unwrap();
    assert_eq!(
        spawns[0]
            .parts
            .iter()
            .find(|(c, _)| *c == health)
            .map(|(_, b)| codec::from_bytes::<Health>(b).unwrap()),
        Some(Health(7))
    );
    assert!(w.entity(SPAWNED).is_none());
}

const SPAWNED: u32 = worldmesh_core::protocol::SPAWNED_ID_START + 5;

// ---- The spatial layer ----

#[test]
fn each_entity_is_reported_to_the_cell_of_where_it_stands_with_its_visible_components() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    spawn_at(&mut w, 2, 40.0, 10.0, false);
    w.step(1001);
    let reports: BTreeMap<u32, GroupReport> = w.take_reports(1001).into_iter().collect();
    assert_eq!(
        reports[&WEST]
            .entities
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(
        reports[&EAST]
            .entities
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    let parts = &reports[&WEST].entities[0].parts;
    assert_eq!(parts.len(), 1, "only Health is visible (not Walker or Log)");
    assert_eq!(codec::from_bytes::<Health>(&parts[0].1), Some(Health(100)));
    assert!(reports[&WEST].entities[0].player);
}

#[test]
fn events_go_to_the_cell_of_where_they_happened() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, true);
    spawn_at(&mut w, 2, 40.0, 10.0, false);
    let mut env = envelope(Address::Entity { id: 2 }, 1, 1, damage(5));
    env.from.tick = 1000;
    w.receive(env);
    w.step(1001);
    let reports: BTreeMap<u32, GroupReport> = w.take_reports(1001).into_iter().collect();
    assert!(reports[&WEST].events.is_empty());
    let ev = &reports[&EAST].events[0];
    assert_eq!(ev.msg, msg::<Hurt>());
    assert_eq!(
        codec::from_bytes::<Hurt>(&ev.body),
        Some(Hurt { id: 2, hp: 95 })
    );
}

/// What an asking entity found: (id, local, its health if it could read it).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
struct Found(Vec<(u32, bool, Option<i32>)>);
impl Component for Found {
    const NAME: &'static str = "found";
}

/// The test game plus a kind that asks what's near every tick and remembers what it found.
fn asking_world() -> GroupWorld {
    let mut world: World = crate::test_game::world();
    world
        .kind(
            "asker",
            Bundle::new().with(Health(1)).with(Found::default()),
        )
        .system_for::<Found>(Stage::Last, |me| {
            me.query(6.0);
            let found = me
                .nearby(6.0)
                .into_iter()
                .map(|n| (n.id, n.local, n.get::<Health>().map(|h| h.0)))
                .collect();
            me.insert(Found(found));
        });
    let mut w = GroupWorld::new(1, world.build());
    w.set_assignment(&west_east());
    w
}

#[test]
fn questions_go_with_the_reports_and_answers_show_up_in_nearby() {
    let mut w = asking_world();
    for (id, x) in [(1, 10.0), (2, 12.0)] {
        let n = NewEntity {
            id,
            x,
            y: 10.0,
            player: false,
            kind: 1,
            parts: Vec::new(),
        };
        w.command(GroupCommand {
            to: None,
            kind: GroupCommandKind::Spawn { entities: vec![n] },
        });
    }
    w.step(1001);
    let reports: BTreeMap<u32, GroupReport> = w.take_reports(1001).into_iter().collect();
    assert_eq!(reports[&WEST].queries.len(), 2);
    // The cell answers: entity 9 of group 2 stands next to entity 1.
    let health = (0, bytes::Bytes::from(codec::to_bytes(&Health(50)).unwrap()));
    let seen = Seen {
        id: 9,
        x: 11.0,
        y: 11.0,
        group: 2,
        player: true,
        kind: 0,
        parts: vec![health],
    };
    w.receive_senses(SenseAnswers {
        spatial: WEST,
        tick: 1001,
        answers: vec![SenseAnswer {
            id: 1,
            seen: vec![seen],
        }],
    });
    w.step(1002);
    let found = &w.entity(1).unwrap().get::<Found>().unwrap().0;
    assert_eq!(
        found,
        &vec![(2, true, Some(1)), (9, false, Some(50))],
        "its own group's entity exactly, the other group's from the answer"
    );
}

#[test]
fn only_visible_components_of_others_can_be_read() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, false);
    spawn_at(&mut w, 2, 11.0, 10.0, false);
    let rules = rules();
    let mut record = w.entities.remove(&1).unwrap();
    let others = BTreeMap::from([(2, w.entities.remove(&2).unwrap())]);
    let (mut requests, mut despawned) = (Requests::default(), false);
    let me = Me {
        rules: &rules,
        entity: &mut record.entity,
        meta: &mut record.meta,
        tick: 1,
        group: 1,
        trace: 0,
        others: &others,
        sensed: &[],
        inputs: &[],
        requests: &mut requests,
        despawned: &mut despawned,
    };
    let seen = me
        .find(2)
        .map(|n| (n.get::<Health>(), n.get::<Log>(), n.kind().to_string()));
    assert_eq!(
        seen,
        Some((Some(Health(100)), None, "unit".to_string())),
        "Log isn't visible, so it reads as missing"
    );
}

#[test]
fn a_query_can_ask_for_only_some_components() {
    let mut w = world(1);
    spawn_at(&mut w, 1, 10.0, 10.0, false);
    let rules = rules();
    let mut record = w.entities.remove(&1).unwrap();
    let others = BTreeMap::new();
    let (mut requests, mut despawned) = (Requests::default(), false);
    let mut me = Me {
        rules: &rules,
        entity: &mut record.entity,
        meta: &mut record.meta,
        tick: 1,
        group: 1,
        trace: 0,
        others: &others,
        sensed: &[],
        inputs: &[],
        requests: &mut requests,
        despawned: &mut despawned,
    };
    me.query(
        crate::Query::within(5.0)
            .with::<Health>()
            .nearest(3)
            .target(9),
    );
    me.query(4.0);
    let health = rules.component_id::<Health>().unwrap();
    assert_eq!(requests.queries[0].only, Some(vec![health]));
    assert_eq!(
        (requests.queries[0].nearest, requests.queries[0].target),
        (3, Some(9))
    );
    assert_eq!(
        requests.queries[1].only, None,
        "everything visible by default"
    );
}

#[test]
fn every_handler_for_an_action_runs_in_order_until_one_despawns_the_entity() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc as StdArc;
    let calls = StdArc::new(AtomicU32::new(0));
    let mut world = crate::test_game::world();
    let c = calls.clone();
    world.action(move |_me, _: &Damage, _| {
        c.fetch_add(1, Ordering::Relaxed);
    });
    world.action(|me, &Damage(n): &Damage, _| {
        if n >= 50 {
            me.despawn();
        }
    });
    let c = calls.clone();
    world.action(move |_me, _: &Damage, _| {
        c.fetch_add(100, Ordering::Relaxed);
    });
    let mut w = GroupWorld::new(1, world.build());
    w.set_assignment(&west_east());
    spawn_at(&mut w, 1, 10.0, 10.0, false);
    spawn_at(&mut w, 2, 11.0, 10.0, false);
    for (id, n) in [(1, 5), (2, 50)] {
        let mut env = envelope(Address::Entity { id }, 9, id.into(), damage(n));
        env.from.tick = 1000;
        w.receive(env);
    }
    w.step(1001);
    assert_eq!(
        log(&w, 1),
        vec!["dmg 5 from 9 on 1001"],
        "the test game's own handler ran first"
    );
    assert!(w.entity(2).is_none());
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1 + 100 + 1,
        "entity 2's last handler didn't run"
    );
}

#[test]
fn systems_run_stage_by_stage_across_every_entity() {
    use std::sync::{Arc as StdArc, Mutex};
    let order = StdArc::new(Mutex::new(Vec::new()));
    let mut world = crate::test_game::world();
    for (stage, name) in [(Stage::Last, "last"), (Stage::First, "first")] {
        let order = order.clone();
        world.system(stage, move |me| {
            order.lock().unwrap().push(format!("{name} {}", me.id))
        });
    }
    let mut w = GroupWorld::new(1, world.build());
    spawn_at(&mut w, 1, 10.0, 10.0, false);
    spawn_at(&mut w, 2, 11.0, 10.0, false);
    w.step(1001);
    assert_eq!(
        *order.lock().unwrap(),
        vec!["first 1", "first 2", "last 1", "last 2"]
    );
}

#[test]
fn a_spawn_starts_with_its_kinds_components_plus_its_own() {
    let rules = rules();
    let spawn = Spawn::new("unit", 1.0, 2.0).with(Health(7));
    let (kind, parts) = rules.encode_spawn(&spawn).unwrap();
    let e = rules.new_entity(5, 1.0, 2.0, false, kind, &parts).unwrap();
    assert_eq!(e.get::<Health>(), Some(&Health(7)));
    assert!(e.has::<Walker>() && e.has::<Log>());
    assert!(
        rules
            .encode_spawn(&Spawn::new("dragon", 0.0, 0.0))
            .is_none(),
        "an unknown kind"
    );
}

#[test]
fn plugins_can_each_add_components_to_a_kind() {
    let mut world = crate::test_game::world();
    world.kind(
        "unit",
        Bundle::new().with(Health(55)).with(Found::default()),
    );
    let rules = world.build();
    let e = rules.new_entity(1, 0.0, 0.0, false, 0, &[]).unwrap();
    assert_eq!(e.get::<Health>(), Some(&Health(55)), "replaced");
    assert!(e.has::<Found>() && e.has::<Walker>(), "added, and kept");
    assert_eq!(rules.schema().kinds, vec!["unit"]);
}

// ---- Fake lag ----

#[test]
fn lag_queue_hands_things_over_in_arrival_order_once_they_have_waited() {
    let mut q = LagQueue::default();
    let t0 = Instant::now();
    let ms = |n: u64| Duration::from_millis(n);
    let cmd = |id: u32| {
        Inbound::Command(GroupCommand {
            to: None,
            kind: GroupCommandKind::Leave { id },
        })
    };
    q.push(t0 + ms(30), cmd(3));
    q.push(t0 + ms(10), cmd(1));
    q.push(t0 + ms(20), cmd(2));
    let ids = |items: Vec<Inbound>| -> Vec<u32> {
        items
            .into_iter()
            .map(|i| match i {
                Inbound::Command(GroupCommand {
                    kind: GroupCommandKind::Leave { id },
                    ..
                }) => id,
                _ => 0,
            })
            .collect()
    };
    assert!(
        q.take_due(t0 + ms(100), ms(100)).is_empty(),
        "nothing has waited 100 ms yet"
    );
    assert_eq!(ids(q.take_due(t0 + ms(125), ms(100))), vec![1, 2]);
    assert_eq!(
        ids(q.take_due(t0 + ms(125), ms(100))),
        Vec::<u32>::new(),
        "each once"
    );
    assert_eq!(ids(q.take_due(t0 + ms(130), ms(100))), vec![3]);
    q.push(t0 + ms(140), cmd(4));
    assert_eq!(ids(q.take_due(t0 + ms(140), Duration::ZERO)), vec![4]);
}
