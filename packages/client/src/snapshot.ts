// Reads the gateway's optimized snapshots: binary, and only what changed since the last one.
// The layout is described in the server's `snapshot.rs`; this reads exactly that.
//
// The decoder keeps every entity in view and applies each snapshot to them, then hands back a
// full snapshot, as if it had come as JSON: an entity the server didn't mention simply hasn't
// changed, and neither has a component it didn't mention. Components and events arrive as
// MessagePack with ids; the schema from the welcome turns the ids into names.

import { decode } from '@msgpack/msgpack';

import type { GameEvent, Named, Schema, Snapshot, SnapshotEntity } from './protocol.ts';

export const KIND_SNAPSHOT = 1;
export const KIND_FULL = 2;
const POS = 1;
const GROUP = 2;
const SPATIAL = 4;
const PARTS = 8;
const GONE = 16;

/** Is this binary message an optimized snapshot (rather than JSON)? */
export const isCompact = (buf: ArrayBuffer) => {
  if (buf.byteLength === 0) return false;
  const kind = new Uint8Array(buf, 0, 1)[0];
  return kind === KIND_SNAPSHOT || kind === KIND_FULL;
};

export class SnapshotDecoder<V = Named, E = Named> {
  /** Your own entity's id: its position comes exact (4-byte floats), everyone else's in 2 bytes. */
  me = -1;
  /** Other entities' positions are steps of 1 / posScale tiles (from the welcome). */
  posScale = 512;
  /** The names of the world's components, messages and kinds (from the welcome). */
  schema: Schema = { components: [], messages: [], kinds: [] };
  private known = new Map<number, SnapshotEntity<Named>>();
  private text = new TextDecoder();

  /** Forget everything (the next snapshot after switching formats sends everyone in full). */
  reset() {
    this.known.clear();
  }

  decode(buf: ArrayBuffer): Snapshot<V, E> {
    const v = new DataView(buf);
    const bytes = new Uint8Array(buf);
    let at = 0;
    const u8 = () => v.getUint8(at++);
    const u16 = () => ((at += 2), v.getUint16(at - 2, true));
    const u32 = () => ((at += 4), v.getUint32(at - 4, true));
    const f32 = () => ((at += 4), v.getFloat32(at - 4, true));
    const msgpack = () => {
      const len = u32();
      const value = decode(bytes.subarray(at, at + len));
      at += len;
      return value;
    };
    const name = (list: string[], id: number) => list[id] ?? `#${id}`;
    const fields = (e: SnapshotEntity<Named>) => {
      const mask = u8();
      if (mask & POS) {
        if (e.id === this.me) {
          e.x = f32();
          e.y = f32();
        } else {
          e.x = u16() / this.posScale;
          e.y = u16() / this.posScale;
        }
      }
      if (mask & GROUP) e.group = u16();
      if (mask & SPATIAL) e.spatial = u16();
      if (mask & PARTS) {
        for (let n = u8(); n > 0; n--) {
          const c = u16();
          e.view[name(this.schema.components, c)] = msgpack();
        }
      }
      if (mask & GONE) {
        for (let n = u8(); n > 0; n--) delete e.view[name(this.schema.components, u16())];
      }
    };

    if (u8() === KIND_FULL) this.known.clear();
    const tick = Number(v.getBigUint64(at, true));
    at += 8;
    for (let n = u16(); n > 0; n--) {
      const id = u32();
      const player = (u8() & 1) !== 0;
      const kind = name(this.schema.kinds, u16());
      const e: SnapshotEntity<Named> = { id, x: 0, y: 0, group: 0, spatial: 0, player, kind, view: {} };
      fields(e);
      this.known.set(id, e);
    }
    for (let n = u16(); n > 0; n--) {
      const id = u32();
      // An update for an entity we don't have can only be a leftover from before a reset:
      // read past it.
      fields(this.known.get(id) ?? { id, x: 0, y: 0, group: 0, spatial: 0, player: false, kind: '', view: {} });
    }
    for (let n = u16(); n > 0; n--) this.known.delete(u32());
    const events: GameEvent<Named>[] = [];
    for (let n = u16(); n > 0; n--) {
      const msg = u16();
      events.push({ name: name(this.schema.messages, msg), data: msgpack() });
    }
    const debugLen = u32();
    const debug = debugLen ? JSON.parse(this.text.decode(bytes.subarray(at, at + debugLen))) : {};
    return {
      t: 'snapshot',
      tick,
      // Copies: the decoder keeps changing its own.
      entities: [...this.known.values()].map((e) => ({ ...e, view: { ...e.view } })) as unknown as SnapshotEntity<V>[],
      events: events as unknown as GameEvent<E>[],
      senses: debug.senses ?? [],
      bus: debug.bus ?? [],
      streams: debug.streams ?? [],
    };
  }
}
