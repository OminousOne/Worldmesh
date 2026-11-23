---
title: The browser client
description: Connect a browser to a world with @worldmesh/client, draw what's in view, and predict your own entity.
---

`@worldmesh/client` is the browser side: it connects to a gateway, decodes snapshots (compact binary or JSON) into plain objects with your game's components and events, by name, keeps every entity in view with about a second of positions, tracks the server clock, and predicts your own entity if you give it your movement rules. It doesn't draw anything: use Pixi, Three.js, a canvas, the DOM.

```sh
npm install @worldmesh/client
```

## Connect

```ts
import { WorldmeshClient } from '@worldmesh/client';

// Your game's types, by the names its plugins registered them under.
type Components = { it?: boolean };                    // visible components
type Events = { tagged: { by: number; who: number } };
type Walk = { dx: number; dy: number };                // the input
type Commands = { tag: { target: number } };

const client = new WorldmeshClient<Components, Events, {}, Walk, Commands>(
  { url: 'ws://localhost:8080/ws' },
  {
    welcome: (w) => console.log('I am', w.id, 'in a', w.world.width, 'x', w.world.height, 'world'),
    added: (e) => createSprite(e.state.id, e.state.kind),
    removed: (id) => destroySprite(id),
  },
);
client.on('tagged', ({ by, who }) => showTag(by, who));
client.connect();
```

Type parameters, in order: your visible components, events, welcome, input, commands, and (for prediction) your predicted state. All are optional; without them everything is `Record<string, unknown>`.

The welcome also carries the world's schema (the names of its components, messages and kinds). The client uses it to decode compact snapshots; you rarely need it yourself.

## Draw

Every frame:

```ts
function frame(dtMs: number) {
  client.frame(dtMs); // advances the prediction
  const t = client.renderTime();
  for (const [id, e] of client.entities) {
    const p = client.positionOf(id, t); // yours: predicted; others: interpolated
    if (p) drawAt(id, p.x, p.y, e.state.view);
  }
}
```

`e.state` is the entity as of the newest snapshot: `id`, `x`, `y`, `group`, `spatial`, `player`, `kind` (its kind's name) and `view` (its visible components, by name). A component it doesn't have is missing from `view`, so `'dead' in e.state.view` asks whether it's dead. Don't draw `e.state.x` directly: it jumps once per tick. `positionOf` is smooth.

## Send

```ts
client.sendInput({ dx: 1, dy: 0 });    // what's held from now on; predicted at once
client.sendCommand('tag', { target: 7 }); // by name; the gateway drops names that aren't commands
```

Send an input when it changes, not every frame.

## Events

`client.on(name, fn)` calls `fn` with each event of that name, in the order they happened, and returns a function that stops it. Every snapshot's events are also in `snapshot.events` as `{ name, data }`.

## Predict your own entity

Give the client your movement rules, the same ones the server runs:

```ts
import { WorldmeshClient, type PredictionRules } from '@worldmesh/client';

type State = { x: number; y: number; dir: { x: number; y: number } };

const rules: PredictionRules<State, Walk, Components> = {
  idle: { dx: 0, dy: 0 },
  isIdle: (w) => w.dx === 0 && w.dy === 0,
  // The server's word on you after a tick, plus the input you held at its end.
  fromServer: (e, held) => ({ x: e.x, y: e.y, dir: { x: held.dx, y: held.dy } }),
  // One tick (or the first `untilSub` sub-steps of it), exactly like your server's movement system.
  simulateTick: (s, changes, untilSub) => mySimulateTick(s, changes, untilSub),
  position: (s) => ({ x: s.x, y: s.y }),
};

const client = new WorldmeshClient<Components, Events, {}, Walk, Commands, State>({
  url: 'ws://localhost:8080/ws',
  prediction: rules,
  shouldPredict: (own) => !('dead' in own.view), // optional: not while dead, say
});
```

`simulateTick` must match the server bit for bit. In JavaScript that means rounding with `Math.fround` exactly where Rust does `f32` math; the medieval example's `movement.ts` shows how, and its `npm run parity` script checks it against the Rust code. If they disagree, prediction still works, it just corrects (and glides) more often: `client.predictor.stats.corrections` counts them.

## Settings and testing tools

```ts
client.setSettings({ compact: true, debug: false }); // binary snapshots; no debug traffic
client.fakeLatency = 100;                             // ms each way, on everything
client.fakeJitter = 25;                               // up to this much more, without reordering
```

Compact snapshots are on by default: binary, and only what changed since the last one (an entity standing still costs nothing). JSON is there for reading in the browser's network tab.
