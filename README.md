# Worldmesh

Worldmesh is a networking library written in Rust, with a TypeScript client for the browser, that lets games split the processing of large multiplayer worlds across many servers while keeping it one seamless experience.

**[Documentation](https://ominousone.github.io/Worldmesh/)** · [Run the example](https://ominousone.github.io/Worldmesh/getting-started/quickstart/) · [Your first game](https://ominousone.github.io/Worldmesh/getting-started/first-game/) · [For AI agents](https://ominousone.github.io/Worldmesh/reference/agents/)

> Status: early (0.1). It works end to end and is tested, including thousands of fighting bots across servers. The API will change.

## How it works

You implement one trait, `Game`: what an entity holds, what players send, what entities do to each other, and one tick of one entity. Worldmesh runs it across:

- **Server Entity Groups**, which simulate entities wherever they stand (balanced by count, so a crowd spreads over as many groups as it takes), and hand them to each other live;
- **Spatial Layer Cells**, which own areas of the map and answer "what's near me?", route actions aimed at an area, and build players' views;
- **an orchestrator**, which decides who owns what;
- **gateways**, which hold players' WebSockets;

all talking over [NATS](https://nats.io). An action sent to an entity on another machine, even one moving between servers, lands exactly once on the next tick.

```rust
impl Game for Tag {
    type Data = Player;     // what an entity holds
    type View = Look;       // what others see of it
    type Input = Walk;      // what a player holds down
    type Command = Command; // what a player asks for
    type Action = Action;   // what entities do to each other
    type Event = Event;     // what players see happen
    type Welcome = ();

    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Walk>]) { /* walk */ }

    fn on_command(&self, me: &mut Me<'_, Self>, Command::Tag { target }: Command) {
        if me.data.it {
            me.send(To::Entity(target), Action::YouAreIt); // wherever the target is
        }
    }
    // config, welcome, join, view, on_action: see examples/hello
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(Tag).await // `tag orchestrator`, `tag group`, `tag spatial`, `tag gateway`
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
| [`crates/worldmesh`](crates/worldmesh) | The library |
| [`packages/client`](packages/client) | `@worldmesh/client`: connection, snapshots, interpolation, prediction |
| [`packages/debug`](packages/debug) | `@worldmesh/debug`: the debug panel |
| [`examples/hello`](examples/hello) | Tag: the smallest complete game |
| [`examples/medieval`](examples/medieval) | Swords, bows, fireballs and bots: server, Pixi client, Docker |
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
