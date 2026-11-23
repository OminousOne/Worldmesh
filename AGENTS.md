# Working on Worldmesh

Instructions for AI coding agents (and people) changing this repository. If you're building a
game *on* Worldmesh rather than changing Worldmesh, use the skills in `skills/` and the docs at
https://ominousone.github.io/Worldmesh/ (machine-readable: `/llms.txt`, `/llms-full.txt`).

## Layout

| Path | What |
| --- | --- |
| `crates/worldmesh` | The library games depend on: re-exports everything below and the derives, plus `worldmesh::run` (`cli`) |
| `crates/worldmesh-core` | Shared by every process: `protocol` (every message), `bus` (NATS, MessagePack or JSON), `codec`, `config`, `subjects`, `time`, `stats`, `views` |
| `crates/worldmesh-sim` | Server Entity Groups: the game model (`world`: World, Plugin, Rules; `component`; `entity`; `me`) and simulation (`group`). `test_game` (feature `test-game`) is the library's own test fixture, not for games |
| `crates/worldmesh-macros` | `#[derive(Component)]` and `#[derive(Message)]` |
| `crates/worldmesh-spatial`, `-orchestrator`, `-gateway` | One crate per role; the gateway also has `snapshot` (compact client snapshots) |
| `crates/worldmesh-testing` | `TestWorld` |
| `packages/client` | `@worldmesh/client` (TypeScript): connection, snapshot decoding, interpolation, prediction |
| `packages/debug` | `@worldmesh/debug` (TypeScript): the debug panel, metrics and bus view |
| `examples/hello` | Tag: the smallest complete game (server only) |
| `examples/medieval` | A full game: `server/` (Rust) and `client/` (Pixi), with Docker files |
| `docs` | The documentation site (Astro Starlight); pages in `docs/src/content/docs`, the landing page's hero and map in `docs/src/components`, colors and fonts in `docs/src/styles/theme.css` |
| `skills` | Agent skills for building on Worldmesh (linked from `.claude/skills`) |

## Commands

```sh
cargo test --workspace                 # every Rust test (library, examples)
cargo clippy --workspace --all-targets # must be clean
npm install && npm test && npm run typecheck   # TypeScript packages and the example client
npm run parity -w medieval-client      # TS movement vs Rust, bit for bit (needs cargo)
make docs                              # build the docs site
make example                           # the medieval example in Docker
```

Run all of the first three before committing.

## Rules

- **The library is only the networking.** No game features in `crates/`: no health, movement, inventory or anything a game would define. A feature that only some games need belongs in an example plugin (or, later, an optional plugin crate), never in the core. What an entity gets from the library is its id, position, player flag and kind.
- **Game code runs only in Server Entity Groups.** Cells, the orchestrator and gateways handle components and messages as ids and MessagePack bytes (the gateway also runs the world's `join` function, checks players' inputs and commands against their types, and names things for JSON clients). Keep it that way.
- **Actions are delivered on the next tick, exactly once, wherever the target is.** Anything touching delivery (`worldmesh-sim` `group.rs`: `route`, `forward`, `deliver_envelopes`, `first_delivery`) needs a test in `group/tests.rs` or `testing.rs`, including mid-handoff cases.
- **The snapshot format is shared with TypeScript.** Changing `crates/worldmesh-gateway/src/snapshot.rs` means changing `packages/client/src/snapshot.ts`, then running `WORLDMESH_WRITE_FIXTURES=1 cargo test -p worldmesh-gateway fixture` and the client's tests.
- **Protocol types mirror TypeScript types** in `packages/client/src/protocol.ts` (Rust side: `crates/worldmesh-core/src/protocol.rs`). Change both.
- **MSRV is 1.87.** Don't use newer std APIs.
- **Docs**: when behaviour or API changes, update the matching page in `docs/src/content/docs`, and the skills if they mention it.

## Style

- Comments and docs: plain, direct sentences that say what something is for. No em dashes anywhere (use colons, commas or parentheses).
- Tests read as sentences: `fn a_hit_on_a_unit_moving_between_groups_lands_exactly_once()`.
- Names for the roles are fixed: Server Entity Group (group), Spatial Layer Cell (cell), orchestrator, gateway.
