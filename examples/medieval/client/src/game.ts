// The game's side of the protocol: the components, events and commands its server's plugins
// register (they mirror the Rust types in ../server/src/{movement,combat,bots}.rs), and its
// movement rules for client-side prediction.

import type { PredictionRules, SnapshotEntity } from '@worldmesh/client';

import { simulateTick, type Grid } from './movement.ts';

export type MapData = Grid;
export type Ability = 'sword' | 'bow' | 'fireball';
export type BotMode = 'aggressive' | 'fight_back' | 'wander';

/** A unit's visible components, by name (Combat's: everything else stays on the server). */
export type Components = {
  health?: number;
  /** Present while dead. */
  dead?: { until: number };
  /** Who it's fighting. */
  fighting?: number;
};

/** What the renderer needs of a unit, from its kind and components. */
export type Look = { hp: number; dead: boolean; bot: boolean; target: number | null };

export const lookOf = (e: SnapshotEntity<Components>): Look => ({
  hp: e.view.health ?? 0,
  dead: e.view.dead !== undefined,
  bot: e.kind === 'bot',
  target: e.view.fighting ?? null,
});

export type Target = { entity: number } | { point: [number, number] };

/** The user's group accepted a use and sent the hit. */
export type Used = { tick: number; actor: number; ability: Ability; target: Target; x: number; y: number; group: number; trace: number };

/** The target's group checked its side and applied it (or not). */
export type Effect = {
  tick: number; actor: number; target: number; ability: Ability; damage: number; hp: number;
  outcome: 'hit' | 'killed' | 'out_of_range' | 'blocked';
  from_group: number; to_group: number; delay_ticks: number; forwarded: boolean; trace: number;
};

/** The game's events, by name. */
export type Events = { used: Used; effect: Effect };

/** An event as the drawing code keeps it, tagged with its name. */
export type Happening = ({ k: 'used' } & Used) | ({ k: 'effect' } & Effect);
export type EffectEvent = Extract<Happening, { k: 'effect' }>;

export type Welcome = { map: MapData };

/** The direction to walk in (length at most 1; zero to stop). */
export type Move = { dx: number; dy: number };

/** The game's commands, by name. */
export type Commands = {
  use: { ability: Ability; target: Target };
  spawn_bots: { count: number; mode: BotMode };
  clear_bots: null;
};

/** What prediction keeps about your own unit: where it is and the direction it holds. */
export type MoveState = { x: number; y: number; dir: { x: number; y: number } };

/** Spawned entities (bots) have ids from 2^31 up. */
const SPAWNED_ID_START = 0x80000000;

/** A short name for any unit. */
export const nameOf = (id: number, bot: boolean) =>
  bot || id >= SPAWNED_ID_START ? `Bot ${(id - SPAWNED_ID_START).toString(36)}` : `Player ${id}`;

/** Each unit's body colour, by id. */
const PALETTE = [0xe6194b, 0x3cb44b, 0xffe119, 0x4363d8, 0xf58231, 0x911eb4, 0x42d4f4, 0xf032e6];
export const colorOf = (id: number) => PALETTE[id % PALETTE.length];

/** The server's movement rules (movement.ts is a bit-for-bit port), for the predictor. */
export function movementRules(grid: () => Grid | undefined): PredictionRules<MoveState, Move, Components> {
  return {
    idle: { dx: 0, dy: 0 },
    isIdle: (m) => m.dx === 0 && m.dy === 0,
    fromServer: (e, held) => ({ x: e.x, y: e.y, dir: { x: held.dx, y: held.dy } }),
    simulateTick: (s, changes, untilSub) =>
      simulateTick(grid()!, s.x, s.y, s.dir, changes.map((c) => ({ sub: c.sub, dx: c.input.dx, dy: c.input.dy })), untilSub),
    position: (s) => ({ x: s.x, y: s.y }),
  };
}
