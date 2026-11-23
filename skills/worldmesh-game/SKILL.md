---
name: worldmesh-game
description: Build or change a multiplayer game on the Worldmesh Rust library. Use when implementing the worldmesh::Game trait, writing entity logic with Me (actions, queries, spawns, events), adding abilities or AI, or testing game rules with TestWorld.
---

# Building a game on Worldmesh

Worldmesh splits a multiplayer world across many servers. A game implements one trait, `worldmesh::Game`; Worldmesh runs it in Server Entity Groups (each simulates some entities, wherever they stand), with Spatial Layer Cells answering "what's near me?", an orchestrator placing entities, and gateways holding players' WebSockets, all over NATS.

Full docs: https://ominousone.github.io/Worldmesh/ (everything in one file: `/llms-full.txt`). Working examples: `examples/hello` (tag, 150 lines) and `examples/medieval/server` (combat and bots).

## The shape of a game

```rust
use serde::{Deserialize, Serialize};
use worldmesh::{time, Entity, EntityId, Game, Incoming, Me, Spawn, Timed, To, ViewShape, WorldConfig};

impl Game for MyGame {
    type Data = Unit;       // an entity's state; travels with it between groups
    type View = Look;       // what others see of it (keep small)
    type Input = Move;      // what a player holds from now on, timed to a sub-step
    type Command = Command; // a player's request, for their own entity
    type Action = Action;   // what entities do to each other
    type Event = Event;     // what players see happen
    type Welcome = Welcome; // sent to every player on connect

    fn config(&self) -> WorldConfig;                                   // same in every process
    fn welcome(&self) -> Welcome;
    fn join(&self, player: EntityId) -> Spawn<Unit>;                    // runs in the gateway
    fn view(&self, e: &Entity<Unit>) -> Look;
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Move>]);     // once per entity per tick
    fn on_command(&self, me: &mut Me<'_, Self>, command: Command) {}
    fn on_action(&self, me: &mut Me<'_, Self>, action: Action, from: &Incoming) {}
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(MyGame::new()).await // the command line picks the role
}
```

Every associated type derives `Serialize, Deserialize, Clone` (and `Debug`). Tag enums for readable JSON: `#[serde(tag = "k", rename_all = "snake_case")]`.

## Me: what entity code can do

`me` dereferences to `Entity<Data>`: `me.id`, `me.x`, `me.y`, `me.player`, `me.data`. Move by assigning `me.x`/`me.y`.

- Time: `me.tick()`, `me.group()`, `me.trace()`, `me.position_at(tick)`.
- Do: `me.send(To::Entity(id) | To::Area { x, y, r } | To::Everyone, action) -> trace`, `me.emit(event)`, `me.emit_at(x, y, event)`, `me.emit_involving(event, &[player_ids])`, `me.spawn(x, y, data)`, `me.despawn()`.
- Know: `me.local()`, `me.local_entity(id)` (own group, exact); `me.query(r)` / `me.query_with(r, nearest, target)` (ask; answer next tick); `me.sensed()`, `me.nearby(r)`, `me.find(id)`.

## Rules (follow all of them)

1. Game code runs only in `tick`, `on_command`, `on_action`, through `me`. `&self` is shared and read-only: put every per-entity value in `Data`.
2. Change another entity only by `me.send` an action. Never store state about other entities outside your own `Data`; they may be on another machine.
3. Actions land on the next tick, exactly once, wherever the target is, even mid-handoff. Don't add retries, acks or delays.
4. Split abilities in two: the user's side (`on_command`/`tick`: alive, cooldown, a point in reach) sends; the target's side (`on_action`) checks and applies.
5. On the target's side, judge with `from.from` (sender id, group, x, y, tick: stamped by the sender's group, never by a client) and `me.position_at(from.from.tick)` (where the target stood when it was sent). Never trust positions or ids a client put in a command.
6. Seed randomness from (entity id, tick), e.g. `splitmix64(SEED ^ (u64::from(me.id) << 32) ^ me.tick())`, so an entity decides the same thing in any group.
7. Inputs are "held from now on" and come with a sub-step: apply direction changes at `t.sub` of `config().input_substeps` so client prediction matches exactly. Movement must be deterministic `f32` math.
8. Ask "what's near me?" only from entities that need it, staggered (`(tick + id) % every == 0`), and prune with `query_with` in crowds. A hit already tells the target where the attacker was.
9. Keep `View` small; it goes to every client in view and into every query answer.
10. Spawned entities get ids from `worldmesh::SPAWNED_ID_START` (2^31); players have ids below it.

## Testing (do this before running servers)

```rust
use worldmesh::testing::TestWorld;

let mut w = TestWorld::new(MyGame::new(), 2, 2); // 2 groups, 2 cells, no NATS
let alice = w.join(1);                           // Game::join, first group
let bob = w.join_in(2, 2);                       // another group
let dummy = w.spawn_in(2, 10.5, 10.5, Unit::default());
w.step();
w.command(alice, Command::Attack { target: bob }); // handled next step
w.input(alice, 8, Move { dx: 1.0, dy: 0.0 });      // sub-step 8 of the next tick
w.run(2);                                          // command runs; action lands
assert_eq!(w.entity(bob).unwrap().data.hp, 80);
w.events(); w.view(alice); w.events_seen(alice); w.group_of(bob);
w.hand_off(bob, 1); w.run_until(60, |w| /* condition */ true); w.run_ms(3000);
```

Test both sides of every check, entities in different groups, a handoff in the same step as an action, and that your client's JSON parses (`serde_json::from_str::<Command>(...)`).

## Checklist for a new ability or behaviour

- [ ] User's side in `on_command` (players) or `tick` (AI): validate, set cooldown in `Data`, `me.send`, `me.emit` a "used" event with the returned trace.
- [ ] Target's side in `on_action`: check with `from.from` and `position_at`, apply to `me.data`, `me.emit_involving(event, &[from.from.id])`.
- [ ] Area version: `To::Area { x, y, r }`; `from.area.is_some()` on the receiving side.
- [ ] TestWorld tests across two groups, including out-of-reach.
- [ ] Update the client's types (TypeScript mirrors of `Command`, `Event`, `View`).
