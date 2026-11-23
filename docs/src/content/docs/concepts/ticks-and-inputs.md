---
title: Ticks, inputs and prediction
description: The shared clock, inputs timed to a sub-step, and how the client predicts its own entity exactly.
---

## One clock

Every process ticks on the same clock: tick `n` is `n × tick_ms` milliseconds after a fixed epoch, so processes agree on the current tick without talking to each other (keep their clocks in sync with NTP, as any server would). The rate is set with `--tick-hz` (or `TICK_HZ`), default 10, and must be the same everywhere.

## Inputs at a sub-step

A player's input (`Game::Input`) is "what I'm holding from now on": a direction, buttons. Each tick is split into `WorldConfig::input_substeps` sub-steps (16 in the examples), and every input names the exact tick and sub-step it takes effect at. Your `tick` gets the inputs due during that tick, in order:

```rust
fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Move>]) {
    // inputs[i].sub: the sub-step it takes effect at; inputs[i].input: your Move
}
```

Releasing a key half way through a tick stops you half way through, not at a tick boundary.

The gateway applies each input exactly when it says, if it arrives in time. An input that arrives too late for its moment is applied as soon as possible instead (the next tick, sub-step 0), and inputs more than 5 seconds ahead are pulled in. An input sent for tick 0 means "as soon as possible".

## Prediction

Because the server applies each input at exactly the sub-step the client chose, a client running the same movement rules gets exactly the same positions. `@worldmesh/client` does this for your own entity:

1. It runs your entity ahead of the server clock by the **lead**.
2. Each input is scheduled at the next sub-step on that clock, applied locally at once, and sent with its tick and sub-step.
3. When the server's position for a tick comes back, it's compared with the prediction for that tick. Equal (within 0.0001 tiles): nothing to do. Different (an input arrived late, something pushed you): take the server's position for that tick, replay every input since, and glide to the result instead of jumping.

The lead adapts: the gateway acks each input with how early it arrived, and the lead is set so the worst input of the last 5 seconds would have arrived 15 ms early. Raising it is immediate; lowering it is gradual.

For this to work, your client's movement code has to match the server's bit for bit. The medieval example keeps a TypeScript port of its Rust movement, rounding with `Math.fround` exactly where Rust does `f32` math, and `npm run parity` checks the two against each other on 20,000 random ticks at several tick rates.

## Others: interpolation

Everyone else is drawn slightly in the past (one tick plus 50 ms), blended between the two snapshots around that moment. There's almost always a newer snapshot to blend toward, so others move smoothly even when a snapshot arrives late. A jump of more than 3 tiles between snapshots (a respawn) is drawn as a jump.
