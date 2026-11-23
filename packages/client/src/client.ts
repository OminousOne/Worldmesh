// A connection to a Worldmesh gateway: decodes what it sends, keeps every entity in view with
// a little position history for smooth drawing, tracks the server's clock, and (if you give
// it your movement rules) predicts your own entity.
//
// Others are drawn slightly in the past (one tick plus 50 ms), interpolated between the two
// snapshots around that moment, so there's almost always a newer one to blend toward even
// when one arrives late. Your own entity is drawn ahead, on the predicted clock.

import { Predictor, type PredictionRules } from './predictor.ts';
import type {
  Assignment,
  ClientMsg,
  DebugCommand,
  EntityId,
  GameEvent,
  Named,
  ProcessId,
  ProcessStats,
  ServerMsg,
  Snapshot,
  SnapshotEntity,
  Welcome,
  WorldConfig,
} from './protocol.ts';
import { isCompact, SnapshotDecoder } from './snapshot.ts';

/** A position the server reported, stamped with server time (ms). */
export type Sample = { t: number; x: number; y: number };

/** An entity in view. */
export type Tracked<V> = {
  /** As of the newest snapshot. */
  state: SnapshotEntity<V>;
  /** About a second of positions, for interpolation. */
  samples: Sample[];
  /** Server time (ms) it was last in a snapshot. */
  lastSeen: number;
};

export type Status = 'connecting' | 'connected' | 'disconnected';

export type ClientHandlers<V = Named, E = Named, W = Named> = {
  status?(status: Status): void;
  welcome?(welcome: Welcome<W>): void;
  /** After the snapshot is applied to `entities` (and its events went to `on` listeners). */
  snapshot?(snapshot: Snapshot<V, E>): void;
  added?(entity: Tracked<V>): void;
  removed?(id: EntityId): void;
  /** An entity moved to another Server Entity Group. */
  handoff?(id: EntityId, from: ProcessId, to: ProcessId): void;
  ownership?(assignment: Assignment): void;
  stats?(processes: ProcessStats[]): void;
};

export type ClientOptions<S, I, V> = {
  /** The gateway's WebSocket URL, like `ws://localhost:8080/ws`. */
  url: string;
  /** Optimized (binary, only what changed) snapshots. Default true. */
  compact?: boolean;
  /** Include debug traffic (bus view, "what's near me?" questions). Default false. */
  debug?: boolean;
  /** Your game's movement rules, to predict your own entity. */
  prediction?: PredictionRules<S, I, V>;
  /** Whether to predict your entity right now (say, not while dead). Default: always. */
  shouldPredict?(own: SnapshotEntity<V>): boolean;
  /** Reconnect this long after the connection drops (ms). Default 1000. */
  reconnectMs?: number;
};

/** Position at time t, linearly interpolated between the samples around it. */
export function sampleAt(samples: Sample[], t: number): { x: number; y: number } {
  if (t <= samples[0].t) return samples[0];
  for (let i = samples.length - 1; i >= 0; i--) {
    const a = samples[i];
    if (a.t <= t) {
      const b = samples[i + 1];
      if (!b) return a; // no newer data yet: hold the latest position
      if (Math.hypot(b.x - a.x, b.y - a.y) > 3) return a; // a teleport (respawn): jump, don't slide
      const k = (t - a.t) / (b.t - a.t);
      return { x: a.x + (b.x - a.x) * k, y: a.y + (b.y - a.y) * k };
    }
  }
  return samples[samples.length - 1];
}

/** Fake network delay: latency plus random jitter, without reordering (a real WebSocket
 *  never reorders either). */
function makeDelay(client: { fakeLatency: number; fakeJitter: number }) {
  let lastDue = 0;
  return (fn: () => void) => {
    if (client.fakeLatency === 0 && client.fakeJitter === 0) return fn();
    const due = Math.max(lastDue, performance.now() + client.fakeLatency + Math.random() * client.fakeJitter);
    lastDue = due;
    setTimeout(fn, due - performance.now());
  };
}

/**
 * A connection to a world. The type parameters are your game's: V its visible components by
 * name, E its events by name, W its welcome, I its input, C its commands by name, S the state
 * your prediction rules track. All are optional.
 */
export class WorldmeshClient<V = Named, E = Named, W = Named, I = unknown, C = Named, S = unknown> {
  /** Your entity's id (-1 until the welcome). */
  me = -1;
  tickMs = 100;
  world: WorldConfig | undefined;
  welcome: Welcome<W> | undefined;
  assignment: Assignment | undefined;
  status: Status = 'connecting';
  lastTick = 0;
  readonly entities = new Map<EntityId, Tracked<V>>();
  readonly predictor: Predictor<S, I, V> | undefined;
  /** Estimated (server clock - local clock) in ms, as of the newest snapshot. */
  clockOffset: number | undefined;
  /** Fake one-way delay on everything sent and received (ms), and random extra on top. */
  fakeLatency = 0;
  fakeJitter = 0;
  /** Traffic and time spent on it since the last `takeMeter`. */
  readonly meter = { downBytes: 0, downMsgs: 0, upBytes: 0, upMsgs: 0, busyMs: 0 };

  private socket: WebSocket | undefined;
  private decoder = new SnapshotDecoder<V, E>();
  private text = new TextDecoder();
  private delayIn = makeDelay(this);
  private delayOut = makeDelay(this);
  private closed = false;
  private compact: boolean;
  private debug: boolean;

  private options: ClientOptions<S, I, V>;
  private listeners: ClientHandlers<V, E, W>[] = [];
  private eventListeners = new Map<string, Set<(data: never, event: GameEvent<E>) => void>>();

  constructor(options: ClientOptions<S, I, V>, handlers: ClientHandlers<V, E, W> = {}) {
    this.options = options;
    this.listeners.push(handlers);
    this.compact = options.compact ?? true;
    this.debug = options.debug ?? false;
    if (options.prediction) {
      this.predictor = new Predictor(options.prediction, { substeps: 16, tickMs: this.tickMs, serverClock: () => this.serverClock() });
    }
  }

  /** Also call these handlers (say, a debug panel's). */
  listen(handlers: ClientHandlers<V, E, W>) {
    this.listeners.push(handlers);
  }

  /** Call `fn` for every event named `name`, in the order they happened. Returns a function
   *  that stops it. */
  on<K extends keyof E & string>(name: K, fn: (data: E[K], event: GameEvent<E>) => void): () => void {
    let set = this.eventListeners.get(name);
    if (!set) this.eventListeners.set(name, (set = new Set()));
    const f = fn as (data: never, event: GameEvent<E>) => void;
    set.add(f);
    return () => set.delete(f);
  }

  private emit<K extends keyof ClientHandlers<V, E, W>>(name: K, ...args: Parameters<NonNullable<ClientHandlers<V, E, W>[K]>>) {
    for (const l of this.listeners) (l[name] as ((...a: typeof args) => void) | undefined)?.(...args);
  }

  get settings() {
    return { compact: this.compact, debug: this.debug };
  }

  connect() {
    this.closed = false;
    this.setStatus('connecting');
    const ws = new WebSocket(this.options.url);
    ws.binaryType = 'arraybuffer';
    this.socket = ws;
    ws.onopen = () => {
      this.setStatus('connected');
      this.sendSettings();
    };
    ws.onmessage = (event) => this.receive(event.data as ArrayBuffer | string);
    ws.onclose = () => {
      if (this.socket !== ws) return;
      this.setStatus('disconnected');
      if (!this.closed) setTimeout(() => this.connect(), this.options.reconnectMs ?? 1000);
    };
  }

  close() {
    this.closed = true;
    this.socket?.close();
  }

  /** The newest server tick, as a continuous clock (fractional ticks). */
  serverClock() {
    return (performance.now() + (this.clockOffset ?? 0)) / this.tickMs;
  }

  /** How far in the past others are drawn (ms). */
  interpDelay() {
    return this.tickMs + 50;
  }

  /** The server time (ms) others are drawn at this frame. */
  renderTime() {
    return performance.now() + (this.clockOffset ?? 0) - this.interpDelay();
  }

  /** Where to draw entity `id` this frame: your own on the predicted clock, others in the
   *  recent past. */
  positionOf(id: EntityId, renderTime = this.renderTime()): { x: number; y: number } | undefined {
    if (id === this.me) {
      const own = this.predictor?.position();
      if (own) return own;
    }
    const e = this.entities.get(id);
    return e && sampleAt(e.samples, renderTime);
  }

  /** Call every frame: keeps the prediction up to date. */
  frame(dtMs: number) {
    if (this.clockOffset !== undefined) this.predictor?.frame(dtMs);
  }

  /** You want `input` from now on. Predicted at once if prediction is on. */
  sendInput(input: I): { tick: number; sub: number } {
    const at = this.predictor ? this.predictor.schedule(input) : { tick: 0, sub: 0 };
    this.send({ t: 'input', tick: at.tick, sub: at.sub, input });
    return at;
  }

  /** A command for your entity, by name (handled by the world's handlers for it). */
  sendCommand<K extends keyof C & string>(name: K, command: C[K]) {
    this.send({ t: 'command', name, command });
  }

  /** How you want snapshots. */
  setSettings(settings: { compact?: boolean; debug?: boolean }) {
    this.compact = settings.compact ?? this.compact;
    this.debug = settings.debug ?? this.debug;
    this.sendSettings();
  }

  /** Debug panel controls (the gateway must run with `--allow-debug`). */
  sendDebug(command: DebugCommand) {
    this.send({ t: 'debug', ...command });
  }

  /** The traffic and time spent since the last call, and start counting again. */
  takeMeter() {
    const m = { ...this.meter };
    Object.assign(this.meter, { downBytes: 0, downMsgs: 0, upBytes: 0, upMsgs: 0, busyMs: 0 });
    return m;
  }

  private sendSettings() {
    this.send({ t: 'settings', compact: this.compact, debug: this.debug });
  }

  private send(msg: ClientMsg<I, C>) {
    const ws = this.socket;
    this.delayOut(() => {
      if (ws?.readyState !== WebSocket.OPEN) return;
      const text = JSON.stringify(msg);
      this.meter.upBytes += text.length;
      this.meter.upMsgs++;
      ws.send(text);
    });
  }

  private setStatus(status: Status) {
    this.status = status;
    this.emit('status', status);
  }

  private receive(data: ArrayBuffer | string) {
    const started = performance.now();
    this.meter.downBytes += typeof data === 'string' ? data.length : data.byteLength;
    this.meter.downMsgs++;
    // Optimized snapshots are binary; everything else is JSON. Decoded as they arrive (in
    // order), before any fake latency, since the decoder builds on the previous snapshot.
    let msg: ServerMsg<V, E, W>;
    if (typeof data !== 'string' && isCompact(data)) {
      msg = this.decoder.decode(data);
    } else {
      msg = JSON.parse(typeof data === 'string' ? data : this.text.decode(data)) as ServerMsg<V, E, W>;
      if (msg.t === 'welcome') {
        this.decoder.me = msg.id;
        this.decoder.posScale = msg.pos_scale;
        this.decoder.schema = msg.schema;
      }
    }
    this.meter.busyMs += performance.now() - started;
    this.delayIn(() => {
      const t = performance.now();
      this.handle(msg);
      this.meter.busyMs += performance.now() - t;
    });
  }

  private handle(msg: ServerMsg<V, E, W>) {
    switch (msg.t) {
      case 'welcome':
        // A new session (first connect or a reconnect): start from nothing.
        for (const id of this.entities.keys()) this.emit('removed', id);
        this.entities.clear();
        this.clockOffset = undefined;
        this.me = msg.id;
        this.tickMs = msg.tick_ms;
        this.world = msg.world;
        this.welcome = msg;
        this.predictor?.configure({ tickMs: msg.tick_ms, substeps: msg.world.input_substeps });
        this.emit('welcome', msg);
        break;
      case 'snapshot':
        this.applySnapshot(msg);
        break;
      case 'ownership': {
        const { t: _, ...assignment } = msg;
        this.assignment = assignment;
        this.emit('ownership', assignment);
        break;
      }
      case 'input_ack':
        this.predictor?.onAck(msg.tick, msg.sub, msg.early_ms);
        break;
      case 'stats':
        this.emit('stats', msg.processes);
        break;
    }
  }

  private applySnapshot(msg: Snapshot<V, E>) {
    const { tick } = msg;
    const serverTime = tick * this.tickMs;
    // Track the server clock: jump to the freshest estimate (least network delay), otherwise
    // drift slowly so clock skew is absorbed without jitter.
    const offsetSample = serverTime - performance.now();
    if (this.clockOffset === undefined || offsetSample > this.clockOffset) this.clockOffset = offsetSample;
    else this.clockOffset += (offsetSample - this.clockOffset) * 0.05;

    for (const s of msg.entities) {
      // JSON positions are f32 written as short decimals; round back to the exact f32.
      s.x = Math.fround(s.x);
      s.y = Math.fround(s.y);
      let e = this.entities.get(s.id);
      if (!e) {
        e = { state: s, samples: [], lastSeen: serverTime };
        this.entities.set(s.id, e);
        this.emit('added', e);
      } else if (e.state.group !== s.group && e.state.group) {
        this.emit('handoff', s.id, e.state.group, s.group);
      }
      e.state = s;
      e.lastSeen = serverTime;
      e.samples.push({ t: serverTime, x: s.x, y: s.y });
      while (e.samples.length > 2 && e.samples[0].t < serverTime - 1000) e.samples.shift(); // about a second
      if (s.id === this.me && this.predictor) {
        if (this.options.shouldPredict && !this.options.shouldPredict(s)) {
          if (this.predictor.tick >= 0) this.predictor.reset();
        } else {
          this.predictor.onServer(tick, s);
        }
      }
    }
    // An entity can miss a tick mid-handoff, or step out of view: remove it once it's been gone a while.
    for (const [id, e] of this.entities) {
      if (serverTime - e.lastSeen > 500) {
        this.entities.delete(id);
        this.emit('removed', id);
      }
    }
    this.lastTick = tick;
    for (const ev of msg.events) {
      for (const fn of this.eventListeners.get(ev.name) ?? []) fn(ev.data as never, ev);
    }
    this.emit('snapshot', msg);
  }
}
