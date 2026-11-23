//! The game's rules, run on a whole Worldmesh world in one process ([`TestWorld`]).

use worldmesh::testing::TestWorld;
use worldmesh::{EntityId, GroupId, Spawn, SPAWNED_ID_START};

use crate::bots::{BotMode, Brain, ClearBots, SpawnBots};
use crate::combat::{
    self, Ability, Dead, Effect, Fighting, Health, Outcome, Target, UseAbility, Used, MAX_HP,
    RESPAWN_MS,
};
use crate::map::MapData;
use crate::movement::{self, Move};

fn world(groups: u32) -> TestWorld {
    TestWorld::new(crate::world(), groups, 2)
}

fn map() -> std::sync::Arc<MapData> {
    crate::map()
}

/// (target, outcome, from group, to group, delay in ticks) of every effect in the last step.
fn effects(w: &TestWorld) -> Vec<(EntityId, Outcome, GroupId, GroupId, u64)> {
    w.events::<Effect>()
        .into_iter()
        .map(|e| (e.target, e.outcome, e.from_group, e.to_group, e.delay_ticks))
        .collect()
}

fn hp(w: &TestWorld, id: EntityId) -> u16 {
    w.entity(id)
        .expect("exists")
        .get::<Health>()
        .expect("has health")
        .0
}

fn pos(w: &TestWorld, id: EntityId) -> (f32, f32) {
    let e = w.entity(id).expect("exists");
    (e.x, e.y)
}

/// A hero no player controls, standing at (x, y).
fn dummy(w: &mut TestWorld, group: GroupId, (x, y): (f32, f32)) -> EntityId {
    w.spawn_in(group, Spawn::new("hero", x, y))
}

/// A walkable spot 8.5 to 11.5 tiles from `p`, in plain sight (in bow range).
fn bow_spot(p: (f32, f32)) -> (f32, f32) {
    let map = map();
    let map = &*map;
    (1..63)
        .flat_map(|x| (1..63).map(move |y| (x, y)))
        .filter(|&(x, y)| map.walkable(x, y))
        .map(|(x, y)| movement::tile_center(x, y))
        .find(|&(x, y)| {
            let d = combat::distance(p.0, p.1, x, y);
            (8.5..11.5).contains(&d) && combat::line_of_sight(map, p.0, p.1, x, y)
        })
        .expect("a spot in bow range and in sight")
}

fn use_on(w: &mut TestWorld, player: EntityId, ability: Ability, target: EntityId) {
    w.command(
        player,
        UseAbility {
            ability,
            target: Target::Entity(target),
        },
    );
}

#[test]
fn a_shot_at_a_unit_in_another_group_lands_on_the_next_tick() {
    let mut w = world(2);
    let alice = w.join(1);
    let spot = bow_spot(pos(&w, alice));
    let dummy = dummy(&mut w, 2, spot);
    w.step();
    use_on(&mut w, alice, Ability::Bow, dummy);
    w.step();
    assert!(
        matches!(
            &w.events::<Used>()[..],
            [Used {
                ability: Ability::Bow,
                ..
            }]
        ),
        "the user's side accepted it"
    );
    assert!(w.events::<Effect>().is_empty());
    assert_eq!(
        hp(&w, dummy),
        MAX_HP,
        "nothing changes until the target's side applies it"
    );
    w.step();
    assert_eq!(hp(&w, dummy), MAX_HP - Ability::Bow.stats().damage);
    assert_eq!(effects(&w), vec![(dummy, Outcome::Hit, 1, 2, 1)]);
}

#[test]
fn the_target_side_rejects_what_is_out_of_reach() {
    let mut w = world(2);
    let alice = w.join(1);
    let (fx, fy) = map().nearest_walkable(58, 58);
    let dummy = dummy(&mut w, 2, movement::tile_center(fx, fy)); // the client asked for too much
    use_on(&mut w, alice, Ability::Bow, dummy);
    w.run(2);
    assert_eq!(hp(&w, dummy), MAX_HP);
    assert_eq!(effects(&w)[0].1, Outcome::OutOfRange);
}

#[test]
fn hits_land_on_the_next_tick_even_in_the_same_group() {
    let mut w = world(1);
    let (alice, bob) = (w.join(1), w.join(2)); // a tile apart
    use_on(&mut w, alice, Ability::Sword, bob);
    w.step();
    assert_eq!(hp(&w, bob), MAX_HP, "not the same tick");
    w.step();
    assert_eq!(hp(&w, bob), MAX_HP - Ability::Sword.stats().damage);
    assert_eq!(effects(&w), vec![(bob, Outcome::Hit, 1, 1, 1)]);
}

#[test]
fn cooldowns_hold() {
    let mut w = world(1);
    let (alice, bob) = (w.join(1), w.join(2));
    use_on(&mut w, alice, Ability::Sword, bob);
    use_on(&mut w, alice, Ability::Sword, bob);
    w.run(2);
    assert_eq!(
        hp(&w, bob),
        MAX_HP - Ability::Sword.stats().damage,
        "the second swing was on cooldown"
    );
}

#[test]
fn a_fireball_hits_everyone_in_its_circle_in_every_group_but_not_its_caster() {
    let mut w = world(2);
    let alice = w.join(1);
    let (ax, ay) = pos(&w, alice);
    let spot = bow_spot((ax, ay));
    let (px, py) = (ax + (spot.0 - ax) * 0.5, ay + (spot.1 - ay) * 0.5); // in fireball reach
    let near = dummy(&mut w, 2, (px + 0.5, py));
    let also = dummy(&mut w, 1, (px, py + 0.5));
    let outside = dummy(&mut w, 2, spot);
    w.step();
    w.command(
        alice,
        UseAbility {
            ability: Ability::Fireball,
            target: Target::Point(px, py),
        },
    );
    w.run(2);
    let damage = Ability::Fireball.stats().damage;
    assert_eq!(
        (hp(&w, near), hp(&w, also)),
        (MAX_HP - damage, MAX_HP - damage)
    );
    assert_eq!((hp(&w, outside), hp(&w, alice)), (MAX_HP, MAX_HP));
}

#[test]
fn a_hit_on_a_unit_moving_between_groups_lands_exactly_once() {
    let mut w = world(3);
    let alice = w.join(1);
    let bob = w.join_in(2, 2);
    w.step();
    w.hand_off(bob, 3);
    use_on(&mut w, alice, Ability::Sword, bob);
    w.run(3);
    assert_eq!(w.group_of(bob), Some(3));
    assert_eq!(hp(&w, bob), MAX_HP - Ability::Sword.stats().damage);
}

#[test]
fn the_dead_dont_move_and_come_back_with_full_hp() {
    let mut w = world(1);
    let (alice, bob) = (w.join(1), w.join(2));
    let killed = {
        let mut ok = false;
        for _ in 0..200 {
            use_on(&mut w, alice, Ability::Sword, bob);
            w.step();
            if w.entity(bob).unwrap().has::<Dead>() {
                ok = true;
                break;
            }
        }
        ok
    };
    assert!(killed, "never died");
    let at = pos(&w, bob);
    w.input(bob, 0, Move { dx: 1.0, dy: 0.0 });
    w.step();
    assert_eq!(pos(&w, bob), at, "the dead don't move");
    w.run_ms(RESPAWN_MS);
    assert_eq!(hp(&w, bob), MAX_HP);
}

#[test]
fn bots_in_another_group_fight_back_without_asking_first() {
    let mut w = world(2);
    let alice = w.join(1);
    let (x, y) = pos(&w, alice);
    let bot = w.spawn_in(
        2,
        Spawn::new("bot", x + 1.0, y).with(Brain::new(BotMode::FightBack)),
    );
    w.step();
    use_on(&mut w, alice, Ability::Sword, bot);
    w.run(2);
    assert!(hp(&w, bot) < MAX_HP);
    assert_eq!(
        w.entity(bot).unwrap().get::<Fighting>(),
        Some(&Fighting(alice))
    );
    w.run(2); // the bot swings back, from where it felt the hit come from
    assert!(hp(&w, alice) < MAX_HP);
}

#[test]
fn wanderers_never_fight_back_and_keep_running() {
    let mut w = world(1);
    let alice = w.join(1);
    let (x, y) = pos(&w, alice);
    let bot = w.spawn_in(
        1,
        Spawn::new("bot", x + 1.0, y).with(Brain::new(BotMode::Wander)),
    );
    use_on(&mut w, alice, Ability::Sword, bot);
    w.run(2);
    assert!(hp(&w, bot) < MAX_HP, "it was hit");
    assert!(
        !w.entity(bot).unwrap().has::<Fighting>(),
        "and doesn't hold a grudge"
    );
    let mut moved = 0;
    for _ in 0..97 {
        let before = pos(&w, bot);
        w.step();
        moved += usize::from(pos(&w, bot) != before);
    }
    assert_eq!(hp(&w, alice), MAX_HP, "never hit back");
    assert!(
        moved > 60,
        "runs around most of the time ({moved} of 97 ticks)"
    );
}

#[test]
fn an_aggressive_bot_attacks_whoever_the_spatial_layer_says_is_near() {
    let mut w = world(2);
    let alice = w.join_in(2, 1);
    let (x, y) = pos(&w, alice);
    // In group 1: only the spatial layer can tell it Alice is there. It stays put (no wandering).
    let brain = Brain {
        idle_until: u64::MAX,
        ..Brain::new(BotMode::Aggressive)
    };
    w.spawn_in(1, Spawn::new("bot", x + 1.0, y).with(brain));
    assert!(
        w.run_until(60, |w| hp(w, alice) < MAX_HP),
        "the bot found Alice and hit her"
    );
}

#[test]
fn players_can_add_and_clear_bots() {
    let mut w = world(2);
    let alice = w.join(1);
    let bob = w.join_in(2, 2);
    w.command(
        alice,
        SpawnBots {
            count: 5,
            mode: BotMode::Wander,
        },
    );
    w.run(2);
    let bots: Vec<EntityId> = w
        .entities()
        .iter()
        .filter(|(_, e)| e.has::<Brain>())
        .map(|(_, e)| e.id)
        .collect();
    assert_eq!(bots.len(), 5);
    assert!(bots.iter().all(|&id| id >= SPAWNED_ID_START));
    w.hand_off(bots[0], 2);
    w.run(2);
    w.command(bob, ClearBots);
    w.run(2);
    assert!(
        w.entities().iter().all(|(_, e)| !e.has::<Brain>()),
        "every bot, in every group"
    );
    assert!(
        w.entity(alice).is_some() && w.entity(bob).is_some(),
        "players stay"
    );
}

#[test]
fn players_see_each_other_and_what_happens_near_them() {
    let mut w = world(2);
    let alice = w.join(1);
    let bob = w.join_in(2, 2);
    w.step();
    let seen: Vec<(EntityId, GroupId)> = w.view(alice).iter().map(|s| (s.id, s.group)).collect();
    assert_eq!(seen, vec![(alice, 1), (bob, 2)]);
    use_on(&mut w, bob, Ability::Sword, alice);
    w.run(2);
    assert!(w
        .events_seen::<Effect>(alice)
        .iter()
        .any(|e| e.target == alice));
    let me = w.view(alice).into_iter().find(|s| s.id == alice).unwrap();
    assert_eq!(
        (me.kind.as_str(), me.get::<Health>()),
        ("hero", Some(Health(MAX_HP - Ability::Sword.stats().damage)))
    );
    assert!(!me.has::<Brain>(), "hidden components stay on the server");
}

#[test]
fn a_change_of_direction_mid_tick_moves_exactly_as_the_client_predicts() {
    let mut w = world(1);
    let alice = w.join(1);
    w.step();
    let (x, y) = pos(&w, alice);
    w.input(alice, 4, Move { dx: 1.0, dy: 0.0 });
    w.step();
    let (nx, ny, _) = movement::simulate_tick(&map(), x, y, (0.0, 0.0), &[(4, (1.0, 0.0))]);
    assert_eq!(pos(&w, alice), (nx, ny));
    assert!(
        (nx - x - movement::speed_per_tick() * 0.75).abs() < 1e-4,
        "12 of 16 sub-steps"
    );
}

#[test]
fn players_messages_read_as_plain_json() {
    let rules = crate::world().build();
    let ok = |name: &str, json: &str| {
        rules
            .command_from_json(name, serde_json::from_str(json).unwrap())
            .is_ok()
    };
    assert!(ok("use", r#"{"ability":"bow","target":{"entity":5}}"#));
    assert!(ok(
        "use",
        r#"{"ability":"fireball","target":{"point":[3.5,4]}}"#
    ));
    assert!(ok("spawn_bots", r#"{"count":10,"mode":"aggressive"}"#));
    assert!(ok("clear_bots", "null"));
    assert!(!ok("use", r#"{"ability":"laser","target":{"entity":5}}"#));
    assert!(
        !ok("hit", r#"{"ability":"sword"}"#),
        "an action: players can't send it"
    );
    assert!(rules
        .input_from_json(serde_json::json!({ "dx": 1, "dy": 0 }))
        .is_ok());
    let mv: Move = serde_json::from_str(r#"{"dx":1,"dy":0}"#).unwrap();
    assert_eq!(mv, Move { dx: 1.0, dy: 0.0 });
}

#[test]
fn each_plugin_adds_its_components_to_the_kinds() {
    let schema = crate::world().build().schema();
    assert_eq!(schema.kinds, vec!["hero", "bot"]);
    for name in [
        "heading",
        "health",
        "dead",
        "fighting",
        "cooldowns",
        "brain",
    ] {
        assert!(schema.components.iter().any(|c| c == name), "{name}");
    }
}
