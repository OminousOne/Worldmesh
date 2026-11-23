---
title: TypeScript packages
description: The API of @worldmesh/client and @worldmesh/debug.
---

## `@worldmesh/client`

### `WorldmeshClient<V, E, W, I, C, S>`

Type parameters, all optional: your visible components by name (`V`), events by name (`E`), welcome (`W`), input (`I`), commands by name (`C`), and predicted state (`S`).

```ts
type Components = { health?: number; dead?: { until: number } };
type Events = { effect: { target: number; damage: number } };
type Commands = { use: { ability: string }; clear_bots: null };
```

```ts
new WorldmeshClient(options: ClientOptions<S, I, V>, handlers?: ClientHandlers<V, E, W>)
```

| Option | Default | |
| --- | --- | --- |
| `url` | required | The gateway's WebSocket URL |
| `compact` | `true` | Binary snapshots with only what changed |
| `debug` | `false` | Include debug traffic (questions, bus messages) |
| `prediction` | none | Your movement rules (`PredictionRules`), to predict your own entity |
| `shouldPredict(own)` | always | Whether to predict right now (say, not while dead) |
| `reconnectMs` | `1000` | Reconnect this long after the connection drops |

| Handler | Called |
| --- | --- |
| `status(status)` | `'connecting'`, `'connected'`, `'disconnected'` |
| `welcome(welcome)` | On connect: your id, the world config, your game's welcome values, the schema |
| `snapshot(snapshot)` | Every tick, after `entities` is updated and `on` listeners ran |
| `added(entity)`, `removed(id)` | An entity came into view, or left it |
| `handoff(id, from, to)` | An entity moved to another group |
| `ownership(assignment)` | The assignment changed |
| `stats(processes)` | Once a second |

More handlers can be added later with `client.listen(handlers)`, and listeners for one kind of event with `client.on(name, fn)`.

| Member | |
| --- | --- |
| `connect()`, `close()` | |
| `me` | Your entity's id (-1 until the welcome) |
| `entities` | `Map<id, Tracked<V>>`: `state` (the newest `SnapshotEntity`: `id`, `x`, `y`, `group`, `spatial`, `player`, `kind`, `view`), `samples`, `lastSeen` |
| `positionOf(id, renderTime?)` | Where to draw an entity this frame: yours predicted, others interpolated |
| `frame(dtMs)` | Call every frame (advances prediction) |
| `renderTime()`, `serverClock()` | The server time others are drawn at (ms); the server's tick (fractional) |
| `sendInput(input)` | Predicted at once, sent for the right tick and sub-step |
| `sendCommand(name, data)` | A command for your entity, by name |
| `on(name, fn)` | `fn(data, event)` for every event named `name`; returns a function that stops it |
| `setSettings({ compact?, debug? })` | |
| `sendDebug(command)` | Scaling, regroup, bus encoding, fake lag (`--allow-debug` gateways) |
| `fakeLatency`, `fakeJitter` | Simulated delay on everything sent and received (ms) |
| `predictor` | The `Predictor`, if prediction is on: `enabled`, `lead`, `stats`, `tick`, `predicted` |
| `world`, `welcome`, `assignment`, `tickMs`, `status`, `lastTick` | |
| `takeMeter()` | Bytes and messages both ways, and time spent, since the last call |

### `PredictionRules<S, I, V>`

```ts
interface PredictionRules<S, I, V> {
  idle: I;                                            // the input held before any is sent
  fromServer(entity: SnapshotEntity<V>, held: I): S;  // your state from the server's word, plus the held input
  simulateTick(state: S, changes: Timed<I>[], untilSub?: number): S; // exactly your server's movement
  position(state: S): { x: number; y: number };
  isIdle?(input: I): boolean;                         // default: it is `idle`
}
```

### Also exported

`Predictor`, `SnapshotDecoder` and `isCompact` (to decode snapshots yourself), `sampleAt`, `isSpatial`, and the protocol types: `Welcome`, `Schema`, `Snapshot`, `SnapshotEntity`, `GameEvent`, `Named`, `Assignment`, `ProcessStats`, `SenseDebug`, `BusMsg`, `StreamMsg`, `WorldConfig`, `ViewShape`, `DebugCommand`, `ClientMsg`, `ServerMsg`.

## `@worldmesh/debug`

```ts
new DebugPanel(options: DebugPanelOptions)
```

| Option | |
| --- | --- |
| `client` | Your `WorldmeshClient` |
| `parent` | Where to put the panels (default `document.body`) |
| `keys` | Keyboard shortcuts (default `true`) |
| `onOverlay(overlay, on)` | An overlay was switched |
| `traceNote(event)` | What one of your events (`{ name, data }`) says about its trace, for the bus view |

| Member | |
| --- | --- |
| `overlays` | `{ groups, spatial, senses, view, ghosts }` |
| `set(name, on)` | Switch an overlay, or `compact`, `binaryBus`, `predict`, `metrics`, `bus` |
| `groupColor(id)`, `groupName(id)`, `spatialName(id)`, `processName(id)`, `short(id)` | Matching the legends |
| `groupIndex(id)`, `spatialIndex(id)`, `lagOf(group)` | |
| `addSection(title)` | An element for your own controls |
| `recordFrame(ms)` | Your frame's main-thread time, for the metrics |
| `minimap` | A canvas shown while the view overlay is on |
| `assignment`, `processStats` | The latest assignment and per-process stats |
| `metrics`, `bus` | The `MetricsPanel` and `BusView` |
