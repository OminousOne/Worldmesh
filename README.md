# Worldmesh

Worldmesh is a networking library written in Rust, with a TypeScript client for the browser, that lets games split the processing of large multiplayer worlds across many servers while keeping it one seamless experience.

**[Documentation](https://ominousone.github.io/Worldmesh/)** · [Run the example](https://ominousone.github.io/Worldmesh/getting-started/quickstart/) · [Your first game](https://ominousone.github.io/Worldmesh/getting-started/first-game/) · [For AI agents](https://ominousone.github.io/Worldmesh/reference/agents/)

> Status: early (0.1). It works end to end and is tested, including thousands of fighting bots across servers. The API will change.

## How it works

Worldmesh is the networking, and none of the game. Every entity gets an id, a position and a kind; everything else (health, movement, inventory, AI) is components and code your game adds as plugins. Worldmesh runs them across:

- **Server Entity Groups**, which simulate entities wherever they stand (balanced by count, so a crowd spreads over as many groups as it takes), and hand them to each other live;
- **Spatial Layer Cells**, which own areas of the map and answer "what's near me?", route actions aimed at an area, and build players' views;
- **an orchestrator**, which decides who owns what;
- **gateways**, which hold players' WebSockets;

all talking over [NATS](https://nats.io). An action sent to an entity on another machine, even one moving between servers, lands exactly once on the next tick.

```rust
#[derive(Component, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct It(bool);                  // a component: part of an entity's state

#[derive(Message, Clone, Serialize, Deserialize)]
struct Tag { target: EntityId }   // a command, from a player

#[derive(Message, Clone, Serialize, Deserialize)]
struct YouAreIt;                  // an action, from one entity to another

struct TagGame;

impl Plugin for TagGame {
    fn build(&self, world: &mut World) {
        world
            .visible::<It>()                          // players can see who's it
            .command(|me, tag: &Tag| {
                if me.get::<It>() == Some(&It(true)) {
                    me.send(To::Entity(tag.target), YouAreIt); // wherever the target is
                }
            })
            .action(|me, _: &YouAreIt, from| { /* in reach of `from`? then me.insert(It(true)) */ })
            .kind("player", Bundle::new().with(It(false)))
            .join(|player| Spawn::new("player", 30.5, 32.5));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut world = World::new(WorldConfig::default());
    world.plugin(TagGame);
    worldmesh::run(world).await // `tag orchestrator`, `tag group`, `tag spatial`, `tag gateway`
}
```

In the browser, `@worldmesh/client` interpolates everyone else and predicts your own entity bit for bit with your movement rules; `@worldmesh/debug` shows who simulates what, scales the world live, and draws every message on the bus.

## Try it

```sh
make example   # Docker: NATS, every server role, and the client. Open http://localhost:5173
```

## Repository

| Path | What |
| --- | --- |
| [`crates/worldmesh`](crates/worldmesh) | The library (one crate per role under [`crates/`](crates), put together here) |
| [`packages/client`](packages/client) | `@worldmesh/client`: connection, snapshots, interpolation, prediction |
| [`packages/debug`](packages/debug) | `@worldmesh/debug`: the debug panel |
| [`examples/hello`](examples/hello) | Tag: the smallest complete game, one plugin |
| [`examples/medieval`](examples/medieval) | Swords, bows, fireballs and bots as Movement, Combat and Bots plugins: server, Pixi client, Docker |
| [`docs`](docs) | The documentation site |
| [`skills`](skills) | Agent skills for building on Worldmesh |

```sh
make test     # Rust tests, TypeScript tests and type checks
make parity   # the example client's movement against the Rust code, bit for bit
make docs     # build the docs site
```

See [AGENTS.md](AGENTS.md) for working on Worldmesh itself.

## License

Apache-2.0. See [LICENSE](LICENSE).
