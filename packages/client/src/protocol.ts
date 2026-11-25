// The messages between a browser and a Worldmesh gateway. They mirror the server's
// `protocol.rs`. The game's own types are type parameters:
//
// - V: an entity's visible components, by name (`{ health: number; dead?: { until: number } }`),
// - E: the game's events, by name (`{ hit: { target: number; damage: number } }`),
// - W: the welcome (what the world sends every player on connect, by key),
// - I: the input,
// - C: the game's commands, by name (`{ use: { ability: string }; clear_bots: null }`).
//
// Components and events are decoded from MessagePack or JSON as plain objects with named
// fields.

export type EntityId = number;
/** A Server Entity Group's or Spatial Layer Cell's id (cells have bit 0x8000 set). */
export type ProcessId = number;

export type ViewShape = { shape: 'rect' | 'iso'; half_w: number; half_h: number };

/** The world's size, chunks and view (the server's `WorldConfig`). */
export type WorldConfig = {
  width: number;
  height: number;
  chunk_size: number;
  view: ViewShape;
  input_substeps: number;
};

/** Which groups and Spatial Layer Cells are in use, and which cell owns each chunk. */
export type Assignment = {
  version: number;
  chunk_size: number;
  chunks_x: number;
  chunks_y: number;
  /** Row-major, one per chunk: the Spatial Layer Cell owning it. */
  owners: ProcessId[];
  spatial: ProcessId[];
  groups: ProcessId[];
  spatial_running: number;
  groups_running: number;
  group_hosts: number;
  binary_bus: boolean;
  /** Fake lag per group (debug): [group, ms one way]. */
  group_lag: [ProcessId, number][];
  /** Fake loss (debug): % of envelopes and confirmations every process drops. */
  fake_loss?: number;
};

export type Rate = { msgs: number; bytes: number };

/** One server process's numbers, once a second. */
export type ProcessStats = {
  /** "group", "group-host", "spatial", "gateway", "orchestrator" or "nats". */
  role: string;
  id: number;
  cpu_pct: number | null;
  mem_mb: number | null;
  bus_in: Rate;
  bus_out: Rate;
  ws_in: Rate;
  ws_out: Rate;
  /** Players connected (gateway), entities simulated (group) or held (cell). */
  players: number;
  tick_ms_avg: number | null;
  tick_ms_max: number | null;
  load_pct: number | null;
  actions_local: number;
  actions_sent: number;
  actions_received: number;
  actions_forwarded: number;
  handoffs: number;
  /** Sure deliveries sent again, and given up on, per second. */
  resent?: number;
  lost?: number;
  queries: number;
  views: number;
  areas: number;
  viewers: number;
  /** Bytes published per second by kind of message: [kind, bytes/s]. */
  bus_kinds?: [string, number][];
};

/** A spatial query, for the debug overlay. */
export type SenseDebug = { id: EntityId; x: number; y: number; r: number; found: number; spatial: ProcessId[] };

/** One envelope that crossed the bus (debug). */
export type BusMsg = {
  from: ProcessId;
  to: ProcessId;
  kind: 'handoff' | 'command' | 'input' | 'action' | 'area' | 'broadcast' | 'forward' | 'resend';
  trace: number;
};

/** A per-tick stream between processes (debug): reports, answers, views, viewers. */
export type StreamMsg = { from: ProcessId; to: ProcessId; kind: 'report' | 'senses' | 'view' | 'viewers'; n: number };

/** Any game's components, events or commands, by name, when you don't give their types. */
export type Named = Record<string, unknown>;

/** The names behind the ids the server uses on the wire, in id order. */
export type Schema = { components: string[]; messages: string[]; kinds: string[] };

/** One of the game's events: its name and what it carries. */
export type GameEvent<E = Named> = { [K in keyof E & string]: { name: K; data: E[K] } }[keyof E & string];

/** One entity in a snapshot. */
export type SnapshotEntity<V = Named> = {
  id: EntityId;
  x: number;
  y: number;
  /** The group simulating it and the Spatial Layer Cell whose area it stands in. */
  group: ProcessId;
  spatial: ProcessId;
  /** Controlled by a connected player. */
  player: boolean;
  /** Its kind's name ("hero", "bot", ...). */
  kind: string;
  /** Its visible components, by name. A component it doesn't have is missing. */
  view: V;
};

export type Welcome<W = Named> = {
  t: 'welcome';
  /** Your entity's id. */
  id: EntityId;
  tick_ms: number;
  world: WorldConfig;
  pos_scale: number;
  /** What the world sends every player on connect (`World::welcome`), by key. */
  game: W;
  schema: Schema;
};

export type Snapshot<V = Named, E = Named> = {
  t: 'snapshot';
  tick: number;
  entities: SnapshotEntity<V>[];
  events: GameEvent<E>[];
  senses: SenseDebug[];
  bus: BusMsg[];
  streams: StreamMsg[];
};

export type ServerMsg<V, E, W> =
  | Welcome<W>
  | Snapshot<V, E>
  | ({ t: 'ownership' } & Assignment)
  | { t: 'input_ack'; tick: number; sub: number; early_ms: number }
  | { t: 'stats'; processes: ProcessStats[] };

/** Debug panel controls (the gateway must run with `--allow-debug`). */
export type DebugCommand = {
  set_groups?: number;
  set_spatial?: number;
  regroup?: boolean;
  binary_bus?: boolean;
  /** [group, ms one way]; 0 turns it off. */
  set_lag?: [ProcessId, number];
  /** % of envelopes and confirmations every process drops; 0 turns it off. */
  set_loss?: number;
};

export type ClientMsg<I, C = Named> =
  | { t: 'input'; tick: number; sub: number; input: I }
  | { t: 'command'; name: keyof C & string; command: C[keyof C] }
  | { t: 'settings'; compact: boolean; debug: boolean; stats: boolean }
  | ({ t: 'debug' } & DebugCommand);

/** Spatial Layer Cell ids have this bit set; group ids don't. */
export const SPATIAL_ID_BIT = 0x8000;
export const isSpatial = (id: ProcessId) => id > 0 && (id & SPATIAL_ID_BIT) !== 0;
