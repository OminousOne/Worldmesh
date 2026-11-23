// The messages between a browser and a Worldmesh gateway. They mirror the server's
// `protocol.rs`; the game's own types (views, events, inputs, commands, the welcome) are
// type parameters, decoded from MessagePack or JSON as plain objects with named fields.

export type EntityId = number;
/** A Server Entity Group's or Spatial Layer Cell's id (cells have bit 0x8000 set). */
export type ProcessId = number;

export type ViewShape = { shape: 'rect' | 'iso'; half_w: number; half_h: number };

/** The world's size, chunks and view, from the game's `Game::config`. */
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
  queries: number;
  views: number;
  areas: number;
  viewers: number;
  /** Bytes published per second by kind of message: [kind, bytes/s]. */
  bus_kinds?: [string, number][];
};

/** A "what's near me?" question, for the debug overlay. */
export type SenseDebug = { id: EntityId; x: number; y: number; r: number; found: number; spatial: ProcessId[] };

/** One envelope that crossed the bus (debug). */
export type BusMsg = {
  from: ProcessId;
  to: ProcessId;
  kind: 'handoff' | 'command' | 'input' | 'action' | 'area' | 'broadcast' | 'forward';
  trace: number;
};

/** A per-tick stream between processes (debug): reports, answers, views, viewers. */
export type StreamMsg = { from: ProcessId; to: ProcessId; kind: 'report' | 'senses' | 'view' | 'viewers'; n: number };

/** One entity in a snapshot. */
export type SnapshotEntity<V> = {
  id: EntityId;
  x: number;
  y: number;
  /** The group simulating it and the Spatial Layer Cell whose area it stands in. */
  group: ProcessId;
  spatial: ProcessId;
  /** Controlled by a connected player. */
  player: boolean;
  /** The game's view of it (`Game::view`). */
  view: V;
};

export type Welcome<W> = {
  t: 'welcome';
  /** Your entity's id. */
  id: EntityId;
  tick_ms: number;
  world: WorldConfig;
  pos_scale: number;
  /** The game's welcome (`Game::welcome`). */
  game: W;
};

export type Snapshot<V, E> = {
  t: 'snapshot';
  tick: number;
  entities: SnapshotEntity<V>[];
  events: E[];
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
};

export type ClientMsg<I, C> =
  | { t: 'input'; tick: number; sub: number; input: I }
  | { t: 'command'; command: C }
  | { t: 'settings'; compact: boolean; debug: boolean }
  | ({ t: 'debug' } & DebugCommand);

/** Spatial Layer Cell ids have this bit set; group ids don't. */
export const SPATIAL_ID_BIT = 0x8000;
export const isSpatial = (id: ProcessId) => id > 0 && (id & SPATIAL_ID_BIT) !== 0;
