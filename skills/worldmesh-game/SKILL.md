---
name: worldmesh-game
description: Build or change a multiplayer game on the Worldmesh Rust library. Use when writing plugins (components, commands, actions, events, systems, kinds), entity logic with Me (actions, queries, spawns, events), abilities or AI, or testing game rules with TestWorld.
---

# Building a game on Worldmesh

Worldmesh splits a multiplayer world across many servers. It does the networking and **none of the game**: every entity gets an id, a position, a "controlled by a player" flag and a kind from Worldmesh, and nothing else. Health, movement, inventory, AI: all of it is your code, added as plugins. Don't look for game features in the library.

A game is a `World` built from plugins. Worldmesh runs it in Server Entity Groups (each simulates some entities, wherever they stand), with Spatial Layer Cells answering "what's near me?", an orchestrator placing entities, and gateways holding players' WebSockets, all over NATS.

Full docs: https://ominousone.github.io/Worldmesh/ (everything in one file: `/llms-full.txt`). Working examples: `examples/hello` (tag, one plugin) and `examples/medieval/server` (Movement, Combat and Bots plugins).

## The shape of a game

```rust
use serde::{Deserialize, Serialize};
use worldmesh::{Bundle, Component, EntityId, Incoming, Me, Message, Plugin, Query, Spawn, Stage, To, World, WorldConfig};

#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Health(u16);                         // a component, named "health"

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
struct Cooldowns(Vec<(u8, u64)>);           // hidden unless registered with `visible`

#[derive(Message, Debug, Clone, Serialize, Deserialize)]
#[worldmesh(name = "use")]                  // the default would be "use_ability"
struct UseAbility { slot: u8, target: EntityId }   // a command (from a player)

#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct Hit { damage: u16 }                  // an action (entity to entity)

#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct Hurt { id: EntityId, hp: u16 }       // an event (for screens)

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Move { dx: f32, dy: f32 }            // the input (plain serde type)

struct Combat;

impl Plugin for Combat {
    fn build(&self, world: &mut World) {
        world
            .visible::<Health>()                          // players and other entities can see it
            .component::<Cooldowns>()                     // only this entity's server sees it
            .command(use_ability)                         // fn(&mut Me, &UseAbility)
            .action(take_hit)                             // fn(&mut Me, &Hit, &Incoming)
            .event::<Hurt>()
            .system_for::<Health>(Stage::Last, regen)     // fn(&mut Me), every tick, entities with Health
            .kind("hero", Bundle::new().with(Health(100)).with(Cooldowns::default()));
    }
}

fn world() -> World {
    let mut world = World::new(WorldConfig::default());          // same in every process
    world.plugin(Movement).plugin(Combat)
        .input::<Move>()                                         // one input type per world
        .join(|player| Spawn::new("hero", 30.5, 32.5))           // required; runs in the gateway
        .welcome("map", &map);                                   // sent to every player on connect
    world
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(world()).await // the command line picks the role
}
```

Components and messages derive `Component` / `Message` plus `Serialize, Deserialize, Clone`. Names default to snake_case of the type; `#[worldmesh(name = "...")]` overrides. Two components or messages can't share a name (the world panics when built).

- `kind(name, bundle)` again with an existing name **adds** components to that kind, so each plugin contributes its own components to shared kinds ("hero", "bot").
- A message is exactly one of command, action or event. Commands and actions can have **any number of handlers**, across plugins, run in registration order (stopping if one despawns the entity).
- Systems run in stages: `Stage::First` (decide, AI), `Stage::Update` (do, movement), `Stage::Last` (clean up, respawn, ask what's near). Each stage runs across all entities before the next. Commands and actions are handled before `First`.

## Me: what entity code can do

`me` dereferences to `Entity`: `me.id`, `me.x`, `me.y`, `me.player`, `me.get::<T>()`, `me.get_mut::<T>()`, `me.has::<T>()`. Move by assigning `me.x`/`me.y`.

- Time: `me.tick()`, `me.group()`, `me.kind()`, `me.config()`, `me.trace()`, `me.position_at(tick)`.
- Inputs (in systems): `for t in me.inputs::<Move>() { t.sub, t.input }`.
- Change itself: `me.insert(c)`, `me.remove::<T>()`, `me.get_mut::<T>()`, `me.despawn()`.
- Do: `me.send(To::Entity(id) | To::Area { x, y, r } | To::Everyone, action) -> trace`, `me.emit(event)`, `me.emit_at(x, y, event)`, `me.emit_involving(event, &[player_ids])`, `me.spawn(Spawn::new("bot", x, y).with(c))`.
- Know: `me.local()` (own group, exact); `me.query(r)` or `me.query(Query::within(r).nearest(n).target(id).with::<T>())` (ask; answer next tick); `me.sensed()`, `me.nearby(r)`, `me.find(id)`. These give `Nearby` (`id`, `x`, `y`, `group`, `player`, `local`, `kind()`, `get::<T>()`, `has::<T>()`), which only read **visible** components.

## Rules (follow all of them)

1. Worldmesh is only the networking. Every game feature (even walking) is a component plus a system or handler in your plugin.
2. Game code runs only in systems and command/action handlers, through `me`, and **changes only the entity it runs for**. Plugins are shared and read-only: put every per-entity value in a component.
3. Change another entity only by `me.send` an action. Never store state about other entities outside your own components; they may be on another machine.
4. Actions land on the next tick, wherever the target is, even mid-handoff, and are never applied twice. Don't add retries, acks or delays.
5. Split abilities in two: the user's side (command handler or system: alive, cooldown, a point in reach) sends; the target's side (action handler) checks and applies.
6. On the target's side, judge with `from.from` (sender id, group, x, y, tick: stamped by the sender's group, never by a client) and `me.position_at(from.from.tick)`. Never trust positions or ids a client put in a command.
7. Plugins meet through components: one writes a component the other reads (medieval: Combat inserts `Immobile` on death, Movement skips immobile units; Combat writes `LastHit`, Bots reads it), or registers another handler for the same action. Expose a `pub fn` (like medieval's `combat::use_ability`) when another plugin should act exactly as a player would.
8. Seed randomness from (entity id, tick), e.g. `splitmix64(SEED ^ (u64::from(me.id) << 32) ^ me.tick())`, so an entity decides the same thing in any group.
9. Inputs are "held from now on" and come with a sub-step: apply direction changes at `t.sub` of `me.config().input_substeps` so client prediction matches exactly. Movement must be deterministic `f32` math.
10. Make visible only what others need, and ask only for what you'll read (`Query::with::<T>()`): answers dominate bus traffic in crowds. Ask only from entities that need it, staggered (`(tick + id) % every == 0`).
11. Spawned entities get ids from `worldmesh::SPAWNED_ID_START` (2^31); players have ids below it.

## Testing (do this before running servers)

```rust
use worldmesh::testing::TestWorld;

let mut w = TestWorld::new(world(), 2, 2);          // 2 groups, 2 cells, no NATS
let alice = w.join(1);                              // the world's join, first group
let bob = w.join_in(2, 2);                          // another group
let dummy = w.spawn_in(2, Spawn::new("hero", 10.5, 10.5));
w.step();
w.command(alice, UseAbility { slot: 0, target: bob }); // handled next step
w.input(alice, 8, Move { dx: 1.0, dy: 0.0 });          // sub-step 8 of the next tick
w.run(2);                                              // command runs; action lands
assert_eq!(w.entity(bob).unwrap().get::<Health>(), Some(&Health(80)));
w.events::<Hurt>(); w.event_names(); w.view(alice); w.events_seen::<Hurt>(alice); w.group_of(bob);
w.view(alice)[0].get::<Health>();                       // what a player sees (visible components)
w.hand_off(bob, 1); w.run_until(60, |w| /* condition */ true); w.run_ms(3000);
w.rules().command_from_json("use", serde_json::json!({ "slot": 0, "target": 2 })); // what the gateway accepts
```

Test both sides of every check, entities in different groups, a handoff in the same step as an action, what players can see, and that your client's JSON parses under the name it sends.

## Checklist for a new ability or behaviour

- [ ] Components for its state (hidden unless others must see it), added to the kinds that need them.
- [ ] User's side in a command handler (players) or a system (AI): validate, update a cooldown component, `me.send`, `me.emit` a "used" event with the returned trace.
- [ ] Target's side in an action handler: check with `from.from` and `position_at`, change `me`'s own components, `me.emit_involving(event, &[from.from.id])`.
- [ ] Area version: `To::Area { x, y, r }`; `from.area.is_some()` on the receiving side.
- [ ] TestWorld tests across two groups, including out-of-reach.
- [ ] Update the client's types (TypeScript maps of component, event and command names).
