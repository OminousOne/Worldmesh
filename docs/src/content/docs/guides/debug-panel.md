---
title: The debug panel
description: Watch and change how the world is split while you play, with @worldmesh/debug.
---

`@worldmesh/debug` is a panel for development: it shows who simulates what and whose area is whose, scales the world, adds fake lag to one server at a time, and draws every message crossing the bus.

```ts
import { DebugPanel } from '@worldmesh/debug';

const panel = new DebugPanel({
  client, // your WorldmeshClient
  onOverlay: (overlay, on) => redrawOverlays(), // groups, spatial, senses, view, ghosts
});
```

The gateway must run with `--allow-debug` for the scaling and lag controls to work. Don't use that flag in production: any connected player could use them.

## What it shows

| Section | What's there |
| --- | --- |
| Server Entity Groups | Groups in use (type any number), how many entities each holds, regroup by position, fake lag for one group |
| Spatial Layer Cells | Cells in use, how many entities each holds and players each serves, questions per second |
| Network | Compact or JSON snapshots, MessagePack or JSON on the bus, bytes per second both ways |
| Prediction | On or off, how far ahead you run, how early your last input arrived, corrections |
| Metrics | CPU, memory, traffic and load per process, over the last minute |
| Bus traffic | Every process around NATS, messages moving between them, recent actions traced hop by hop |

Your game can add its own sections with `panel.addSection(title)`, which returns an element to fill (the medieval example puts its bot controls there).

## Keys

| Key | What it toggles |
| --- | --- |
| O | Colour entities by group |
| L | Spatial Layer Cell areas |
| N | "What's near me?" questions |
| V | Your view, on a minimap |
| G | Raw server positions (ghosts) |
| P | Prediction |
| T | Compact snapshots |
| I | MessagePack on the bus |
| B | Bus view |
| M | Metrics |

## Drawing overlays

The panel doesn't draw on your world: that's your renderer. It keeps the overlays' state in `panel.overlays` (`groups`, `spatial`, `senses`, `view`, `ghosts`) and calls `onOverlay` when one changes. For colours and names that match its legends, use `panel.groupColor(id)`, `panel.groupName(id)`, `panel.spatialName(id)` and `panel.short(id)`. `panel.minimap` is a canvas shown while the view overlay is on.

The data for overlays comes from the client: `entity.state.group` and `.spatial`, `client.assignment` (which cell owns each chunk), the `senses` in each snapshot (questions asked, while debug traffic is on), and the client's `handoff` handler.

## Tracing actions

The bus view lists recent actions hop by hop (`Player 3 bow → Bot k3x: Gateway → Group 1 → Group 2 · hit 12`). Tell it which of your events start and end a trace:

```ts
new DebugPanel({
  client,
  traceNote: (ev) =>
    ev.k === 'used'
      ? { trace: ev.trace, start: { label: `${name(ev.actor)} ${ev.ability}`, group: ev.group } }
      : { trace: ev.trace, result: { text: ev.outcome, landed: ev.outcome === 'hit' } },
});
```

Every envelope one action leads to shares its trace id; `me.send` returns it, and `Incoming::trace` carries it on the receiving side.
