---
title: The browser client
description: Connect a browser to a world with @worldmesh/client, draw what's in view, and predict your own entity.
---

`@worldmesh/client` is the browser side of a world. It:

- connects to a gateway and decodes snapshots into plain objects, with your components and events by name,
- keeps every entity in view with about a second of positions, and tracks the server clock,
- predicts your own entity, if you give it your movement rules.

It draws nothing: use Pixi, Three.js, a canvas or the DOM.

```sh
npm install @worldmesh/client
```

## Connect

```ts
import { WorldmeshClient } from '@worldmesh/client';

// Your game's types, by the names its plugins registered them under.
type Components = { it?: boolean }; // visible components
type Events = { tagged: { by: number; who: number } };
type Walk = { dx: number; dy: number }; // the input
type Commands = { tag: { target: number } };

const client = new WorldmeshClient<Components, Events, {}, Walk, Commands>(
  { url: 'ws://localhost:8080/ws' },
  {
    welcome: (w) => console.log('I am', w.id, 'in a', w.world.width, 'tile world'),
    added: (e) => createSprite(e.state.id, e.state.kind),
    removed: (id) => destroySprite(id),
  },
);
client.on('tagged', ({ by, who }) => showTag(by, who));
client.connect();
```

The type parameters, in order, are your visible components, events, welcome, input, commands and predicted state. All are optional; without them everything is `Record<string, unknown>`.

## Draw

Every frame:

```ts
function frame(dtMs: number) {
  client.frame(dtMs); // advances the prediction
  const t = client.renderTime();
  for (const [id, e] of client.entities) {
    // Yours: predicted. Everyone else: interpolated.
    const p = client.positionOf(id, t);
    if (p) drawAt(id, p.x, p.y, e.state.view);
  }
}
```

`e.state` is the entity as of the newest snapshot: `id`, `x`, `y`, `group`, `spatial`, `player`, `kind` and `view`, its visible components by name. A component it doesn't have is missing from `view`, so `'dead' in e.state.view` asks whether it's dead.

Draw at `positionOf`, not `e.state.x`: the state jumps once per tick, `positionOf` is smooth. [Ticks, inputs and prediction](/Worldmesh/concepts/ticks-and-inputs/#prediction) has a demo of both.

## Send

```ts
// What's held from now on. Predicted at once.
client.sendInput({ dx: 1, dy: 0 });
// By name. The gateway drops names that aren't commands.
client.sendCommand('tag', { target: 7 });
```

Send an input when it changes, not every frame.

## Listen for events

`client.on(name, fn)` calls `fn` with each event of that name, in the order they happened, and returns a function that stops listening. Every snapshot's events are also in `snapshot.events`, as `{ name, data }`.

## Predict your own entity

Give the client the same movement rules the server runs:

```ts
import { WorldmeshClient, type PredictionRules } from '@worldmesh/client';

type State = { x: number; y: number; dir: { x: number; y: number } };

const rules: PredictionRules<State, Walk, Components> = {
  idle: { dx: 0, dy: 0 },
  isIdle: (w) => w.dx === 0 && w.dy === 0,
  // The server's word on you after a tick, plus the input you held at its end.
  fromServer: (e, held) => ({ x: e.x, y: e.y, dir: { x: held.dx, y: held.dy } }),
  // One tick, or its first `untilSub` sub-steps: exactly your server's movement.
  simulateTick: (s, changes, untilSub) => mySimulateTick(s, changes, untilSub),
  position: (s) => ({ x: s.x, y: s.y }),
};

const client = new WorldmeshClient<Components, Events, {}, Walk, Commands, State>({
  url: 'ws://localhost:8080/ws',
  prediction: rules,
  // Optional: not while dead, say.
  shouldPredict: (own) => !('dead' in own.view),
});
```

`simulateTick` must match the server bit for bit. In JavaScript, that means rounding with `Math.fround` exactly where Rust does `f32` math. The medieval example's `movement.ts` shows how, and its `npm run parity` script checks it against the Rust code.

If the two disagree, prediction still works: it just corrects (and glides) more often. `client.predictor.stats.corrections` counts the corrections.

## Settings and testing tools

```ts
// Binary snapshots, no debug traffic.
client.setSettings({ compact: true, debug: false });
// Fake delay on everything sent and received, in ms each way.
client.fakeLatency = 100;
// Up to this much more, without reordering.
client.fakeJitter = 25;
```

Compact snapshots are on by default: binary, with only what changed since the last one, so an entity standing still costs nothing. JSON is there for reading in the browser's network tab.
