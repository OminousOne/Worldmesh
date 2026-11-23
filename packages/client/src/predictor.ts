// Client-side prediction for your own entity.
//
// Your entity is simulated locally ahead of the server, with the same rules the server runs
// (you supply them), so it responds to input at once. Each input is scheduled at an exact
// sub-step of an exact tick, and the server applies it at exactly that moment, so when the
// server's position for a tick comes back it should match the prediction bit for bit. When it
// doesn't (an input arrived late, something pushed you), the predictor takes the server's
// position for that tick, replays every input sent since, and glides to the result.
//
// How far ahead to run (the lead) follows how early inputs reach the server: the gateway acks
// every input with how early it arrived, and the lead is picked so the worst recent input
// would have arrived EARLY_TARGET_MS early.
//
// Inputs are "what's held from now on" (a direction, buttons): the latest one stays in effect
// until the next.

import type { SnapshotEntity } from './protocol.ts';

/** An input and the sub-step of its tick it takes effect at. */
export type Timed<I> = { sub: number; input: I };

/** Your game's movement rules, the same ones the server runs. */
export interface PredictionRules<S, I, V = unknown> {
  /** The input held when nothing has been sent yet (standing still). */
  idle: I;
  /** Your full state from the server's word on your entity after a tick, and the input
   *  held at the end of that tick (as you sent it). */
  fromServer(entity: SnapshotEntity<V>, held: I): S;
  /** One tick from `state`, with the inputs that take effect during it (in sub-step order).
   *  With `untilSub`, stop that many (possibly fractional) sub-steps in, for drawing between
   *  ticks. Must match the server's `Game::tick` exactly. */
  simulateTick(state: S, changes: Timed<I>[], untilSub?: number): S;
  /** Where `state` is, in tiles. */
  position(state: S): { x: number; y: number };
  /** Whether holding `input` means standing still (default: it is `idle`). Used to adjust the
   *  lead where it can't be seen. */
  isIdle?(input: I): boolean;
}

/** A prediction this close to the server's is right. */
const CORRECTION_EPSILON = 1e-4;
/** Corrections bigger than this (respawns, teleports) snap instead of gliding. */
const SNAP_DISTANCE = 2;
/** Aim for inputs to reach the server this early. */
const EARLY_TARGET_MS = 15;
/** The lead follows the worst input of this long. */
const LEAD_WINDOW_MS = 5000;
const MAX_LEAD_MS = 2000;
/** How fast a correction's leftover offset fades (ms time constant). */
const SMOOTH_MS = 80;

export type PredictorOptions = {
  /** Sub-steps per tick (`WorldConfig::input_substeps`). */
  substeps: number;
  tickMs: number;
  /** The server's clock, in (fractional) ticks. */
  serverClock: () => number;
};

export class Predictor<S, I, V = unknown> {
  enabled = true;
  /** How far ahead of the server clock your entity runs, in (fractional) ticks. */
  lead: number;
  readonly stats = { corrections: 0, lastError: 0, early: NaN };

  private leadTarget: number;
  private substeps: number;
  private tickMs: number;
  private serverClock: () => number;
  /** Inputs sent but not acknowledged yet, with the lead they were sent at. */
  private inFlight: { tick: number; sub: number; lead: number }[] = [];
  /** Recent acks: how early each input would have arrived with zero lead (ms). */
  private earlySamples: { at: number; base: number }[] = [];
  /** Last tick simulated locally (-1: not started yet). */
  private predTick = -1;
  private state: S | undefined;
  /** Predicted position after each recent tick, to compare with the server's. */
  private history = new Map<number, { x: number; y: number }>();
  /** Inputs sent that the server hasn't simulated past yet, in time order. */
  private sent: { tick: number; sub: number; input: I }[] = [];
  /** The input held before the first of `sent`. */
  private base: I;
  /** The input last asked for. */
  private wanted: I;
  private lastChangeAt = 0;
  /** Visual offset left over from a correction; decays to zero so corrections glide. */
  private smooth = { x: 0, y: 0 };

  private rules: PredictionRules<S, I, V>;

  constructor(rules: PredictionRules<S, I, V>, options: PredictorOptions) {
    this.rules = rules;
    this.substeps = options.substeps;
    this.tickMs = options.tickMs;
    this.serverClock = options.serverClock;
    this.lead = this.leadTarget = 200 / this.tickMs; // 200 ms ahead to start; adapts after the first inputs
    this.base = this.wanted = rules.idle;
  }

  /** New world settings (on every welcome). */
  configure(options: Partial<PredictorOptions>) {
    if (options.substeps) this.substeps = options.substeps;
    if (options.serverClock) this.serverClock = options.serverClock;
    if (options.tickMs) {
      this.tickMs = options.tickMs;
      this.lead = this.leadTarget = 200 / this.tickMs;
    }
    this.earlySamples = [];
    this.wanted = this.rules.idle;
    this.reset();
  }

  /** Your entity's clock: the server's plus the lead, in ticks. */
  clock() {
    return this.serverClock() + this.lead;
  }

  /** The last tick simulated locally (-1 before the first server position). */
  get tick() {
    return this.predTick;
  }

  /** Start over from the next server position. */
  reset() {
    this.predTick = -1;
    this.state = undefined;
    this.history.clear();
    this.sent = [];
    this.inFlight = [];
    this.base = this.wanted;
    this.smooth.x = this.smooth.y = 0;
  }

  /** You want `input` from now on. Returns when to send it for: the next sub-step on your
   *  entity's clock (tick 0: as soon as the server can, when not predicting). */
  schedule(input: I): { tick: number; sub: number } {
    this.wanted = input;
    if (!this.enabled || this.predTick < 0) {
      // Not predicting (or not started yet): the server applies it as soon as it can, and
      // prediction (re)starts from this input.
      this.sent = [];
      this.base = input;
      return { tick: 0, sub: 0 };
    }
    // Right now on the predicted clock, rounded up to the next sub-step (and never inside a
    // tick already simulated). Server and client both apply it at exactly that sub-step.
    const n = this.substeps;
    const at = Math.max(Math.ceil(Math.max(this.clock(), this.predTick) * n), this.lastChangeAt);
    this.lastChangeAt = at;
    const tick = Math.floor(at / n) + 1;
    const sub = at % n;
    const last = this.sent[this.sent.length - 1];
    if (last && last.tick === tick && last.sub === sub) this.sent.pop(); // same moment: replaces it
    this.sent.push({ tick, sub, input });
    this.inFlight.push({ tick, sub, lead: this.lead });
    return { tick, sub };
  }

  /** The gateway's ack: the input for (tick, sub) arrived `earlyMs` before its moment. */
  onAck(tick: number, sub: number, earlyMs: number) {
    if (tick === 0 || !this.enabled) return; // sent without prediction
    let sent: { tick: number; sub: number; lead: number } | undefined;
    while (this.inFlight.length && !sent) {
      const s = this.inFlight.shift()!;
      if (s.tick === tick && s.sub === sub) sent = s;
    }
    if (!sent) return;
    this.stats.early = earlyMs;
    const now = performance.now();
    // Independent of the lead it was sent with, so samples from before a lead change still count.
    this.earlySamples.push({ at: now, base: earlyMs - sent.lead * this.tickMs });
    while (this.earlySamples[0].at < now - LEAD_WINDOW_MS) this.earlySamples.shift();
    const worst = Math.min(...this.earlySamples.map((e) => e.base));
    const needed = Math.min(MAX_LEAD_MS / this.tickMs, Math.max(0, (EARLY_TARGET_MS - worst) / this.tickMs));
    if (needed > this.leadTarget) {
      this.leadTarget = needed; // too close or late: run further ahead right away
    } else if (needed < this.leadTarget - 0.05 && now - this.earlySamples[0].at >= 1500) {
      this.leadTarget = needed; // comfortably early for a while: tighten
    }
  }

  /** The server's word on your entity after tick `tick`: check the prediction for that tick
   *  and fix it if needed. */
  onServer(tick: number, entity: SnapshotEntity<V>) {
    if (!this.enabled) return;
    for (const k of this.history.keys()) if (k < tick) this.history.delete(k);
    this.forgetBefore(tick);
    const restart = () => {
      this.predTick = tick;
      this.state = this.rules.fromServer(entity, this.heldAfter(tick));
      this.history.set(tick, this.rules.position(this.state));
    };
    if (this.predTick < tick) {
      // Not predicting yet, or fell behind the server: start over from its position.
      this.history.clear();
      restart();
      return;
    }
    const mine = this.history.get(tick);
    const error = mine ? Math.hypot(mine.x - entity.x, mine.y - entity.y) : Infinity;
    if (error <= CORRECTION_EPSILON) return; // the prediction was right

    // Wrong: take the server's state for that tick and replay every tick after it.
    this.stats.corrections++;
    this.stats.lastError = Number.isFinite(error) ? error : 0;
    const before = this.rawPosition();
    const end = this.predTick;
    restart();
    while (this.predTick < end) this.simulateNext();
    this.glide(before, this.rawPosition());
  }

  /** Every frame: adjust the lead and simulate up to your entity's clock. */
  frame(dtMs: number) {
    const decay = Math.exp(-dtMs / SMOOTH_MS);
    this.smooth.x *= decay;
    this.smooth.y *= decay;
    if (!this.enabled) return;
    this.easeLead(dtMs);
    this.advance();
  }

  /** Where to draw your entity this frame (undefined when not predicting). */
  position(): { x: number; y: number } | undefined {
    if (!this.enabled) return undefined;
    const p = this.rawPosition();
    return p && { x: p.x + this.smooth.x, y: p.y + this.smooth.y };
  }

  /** Your predicted state after the last simulated tick. */
  get predicted(): S | undefined {
    return this.state;
  }

  private changesIn(tick: number) {
    return this.sent.filter((c) => c.tick === tick).map(({ sub, input }) => ({ sub, input }));
  }

  /** The input held at the end of `tick`, by what was sent. */
  private heldAfter(tick: number): I {
    for (let i = this.sent.length - 1; i >= 0; i--) if (this.sent[i].tick <= tick) return this.sent[i].input;
    return this.base;
  }

  /** Drop inputs the server has simulated past. */
  private forgetBefore(tick: number) {
    let n = 0;
    while (n < this.sent.length && this.sent[n].tick < tick) n++;
    if (n === 0) return;
    this.base = this.sent[n - 1].input;
    this.sent = this.sent.slice(n);
  }

  private simulateNext() {
    this.predTick++;
    this.state = this.rules.simulateTick(this.state!, this.changesIn(this.predTick));
    this.history.set(this.predTick, this.rules.position(this.state));
  }

  private advance() {
    if (this.predTick < 0) return;
    const target = Math.floor(this.clock());
    if ((target - this.predTick) * this.tickMs > 2000) {
      // Way behind (the tab was in the background): start over from the next snapshot.
      this.predTick = -1;
      this.history.clear();
      return;
    }
    while (this.predTick < target) this.simulateNext();
  }

  /** Where your entity is on its clock: part way into the tick after the last simulated one. */
  private rawPosition(): { x: number; y: number } | undefined {
    if (this.predTick < 0 || !this.state) return undefined;
    const frac = Math.min(1, Math.max(0, this.clock() - this.predTick));
    return this.rules.position(this.rules.simulateTick(this.state, this.changesIn(this.predTick + 1), frac * this.substeps));
  }

  /** Glide from where it was drawn to where it is now, instead of jumping (but snap on big jumps). */
  private glide(before: { x: number; y: number } | undefined, after: { x: number; y: number } | undefined) {
    if (before && after && Math.hypot(before.x - after.x, before.y - after.y) < SNAP_DISTANCE) {
      this.smooth.x += before.x - after.x;
      this.smooth.y += before.y - after.y;
    } else {
      this.smooth.x = this.smooth.y = 0;
    }
  }

  /** Move the lead toward its target. Raising is urgent (every input sent with the old lead
   *  may miss its tick), so jump at once and glide over the visual jump. Lowering is not: run
   *  the clock 10% slower while moving (90% while still, where nothing shows) until it gets
   *  there, so it never runs backward. */
  private easeLead(dtMs: number) {
    if (this.leadTarget > this.lead) {
      const before = this.rawPosition();
      this.lead = this.leadTarget;
      this.advance();
      const after = this.rawPosition();
      if (before && after && Math.hypot(before.x - after.x, before.y - after.y) < SNAP_DISTANCE) {
        this.smooth.x += before.x - after.x;
        this.smooth.y += before.y - after.y;
      }
    } else if (this.leadTarget < this.lead) {
      const idle = this.rules.isIdle ?? ((i: I) => i === this.rules.idle);
      const moving = this.sent.some((c) => c.tick > this.predTick) || !idle(this.heldAfter(this.predTick));
      this.lead = Math.max(this.leadTarget, this.lead - (dtMs / this.tickMs) * (moving ? 0.1 : 0.9));
    }
  }
}
