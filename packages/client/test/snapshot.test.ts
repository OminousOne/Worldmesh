// The decoder reads exactly what the server's encoder writes: the fixture comes from the Rust
// test `the_client_fixture_matches_what_the_encoder_writes`.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { isCompact, SnapshotDecoder } from '../src/snapshot.ts';

type Fixture = {
  me: number;
  pos_scale: number;
  schema: { components: string[]; messages: string[]; kinds: string[] };
  snapshots: { hex: string; tick: number; entities?: unknown[]; events?: unknown[]; senses?: unknown[] }[];
};

const fixture: Fixture = JSON.parse(readFileSync(new URL('./fixtures/snapshots.json', import.meta.url), 'utf8'));
const bytes = (hex: string) => new Uint8Array(hex.match(/../g)!.map((h) => parseInt(h, 16))).buffer;

test('decodes the server encoder output, applying each snapshot to the last', () => {
  const decoder = new SnapshotDecoder();
  decoder.me = fixture.me;
  decoder.posScale = fixture.pos_scale;
  decoder.schema = fixture.schema;
  for (const s of fixture.snapshots) {
    const buf = bytes(s.hex);
    assert.ok(isCompact(buf));
    const got = decoder.decode(buf);
    assert.equal(got.tick, s.tick);
    if (!s.entities) continue;
    const sorted = [...got.entities].sort((a, b) => a.id - b.id);
    assert.deepEqual(sorted, s.entities);
    assert.deepEqual(got.events, s.events);
    assert.deepEqual(got.senses, s.senses);
  }
});

test('entities handed out are copies the decoder does not change later', () => {
  const decoder = new SnapshotDecoder();
  decoder.me = fixture.me;
  decoder.posScale = fixture.pos_scale;
  decoder.schema = fixture.schema;
  const first = decoder.decode(bytes(fixture.snapshots[0].hex));
  const before = JSON.stringify(first.entities);
  decoder.decode(bytes(fixture.snapshots[1].hex));
  assert.equal(JSON.stringify(first.entities), before);
});

test('JSON messages are not mistaken for snapshots', () => {
  assert.equal(isCompact(new TextEncoder().encode('{"t":"welcome"}').buffer), false);
  assert.equal(isCompact(new ArrayBuffer(0)), false);
});
