// The medieval example's client: sends what the player wants, draws what the server says.
//
// @worldmesh/client does the networking: it keeps every unit in view with a little position
// history (others are drawn slightly in the past, blended between snapshots) and predicts
// your own unit with the same movement rules the server runs (movement.ts), reconciling
// whenever the server's answer for a tick differs. @worldmesh/debug is the panel on the right.
// This file is the game: Pixi drawing, combat effects, input, and the debug overlays.
//
// The world is simulated by Server Entity Groups (each owns some units, wherever they are)
// and watched by Spatial Layer Cells (each owns an area and answers "what's near here?"). What
// you see is your view: whatever is on your screen, put together from every Spatial Layer Cell
// your screen covers.

import { Application, BitmapFont, BitmapText, Container, Graphics, Rectangle, Sprite, Text, Texture, UPDATE_PRIORITY, type Renderer } from 'pixi.js';
import { WorldmeshClient, type Assignment, type SenseDebug, type SnapshotEntity } from '@worldmesh/client';
import { DebugPanel, type TraceNote } from '@worldmesh/debug';

import {
  colorOf, lookOf, movementRules, nameOf,
  type Ability, type BotMode, type Commands, type Components, type Events, type Happening, type Look, type MapData, type Move,
  type MoveState, type Target, type Welcome,
} from './game.ts';
import { sanitizeDir, setTickMs, type Grid } from './movement.ts';

declare global {
  interface Window {
    /** Live client state, for automated tests. */
    gameDebug?: {
      x: number;
      y: number;
      group: number;
      spatial: number;
      predicted: { x: number; y: number };
      server: { x: number; y: number };
      predTick: number;
      corrections: number;
      lead: number;
      /** Estimated server clock, in fractional ticks. */
      serverClock?: number;
      /** Every unit drawn, with its screen position (tests). */
      entities?: () => { id: number; name: string; bot: boolean; hp: number; group: number; spatial: number; x: number; y: number; sx: number; sy: number }[];
      target?: number | null;
    };
  }
}

const GATEWAY_URL: string = import.meta.env.VITE_GATEWAY_URL ?? `ws://${location.hostname}:8080/ws`;

// 2:1 isometric diamonds. Placeholder art is all flat shapes.
const TILE_W = 64;
const TILE_H = 32;
const WALL_H = 28;
const GRASS = 0, PATH = 1, WATER = 2, WALL = 3;
const TILE_COLORS: Record<number, [number, number]> = {
  [GRASS]: [0x4f8a3a, 0x548f3e],
  [PATH]: [0xc8b07a, 0xc2aa74],
  [WATER]: [0x2d6aa3, 0x2a659c],
  [WALL]: [0x8a8070, 0x8a8070],
};
/** "What's near me?" questions: neutral, so they never look like a group's colour. */
const SENSE_COLOR = 0xc3c2b7;
const MAX_HP = 100;
/** Same numbers as the server's ability table (../server/src/combat.rs). The client only uses
 *  them to decide when to ask; the servers decide what happens. */
const ABILITIES: Record<Ability, { range: number; cooldownMs: number; radius: number }> = {
  sword: { range: 1.5, cooldownMs: 800, radius: 0 },
  bow: { range: 12, cooldownMs: 1200, radius: 0 },
  fireball: { range: 10, cooldownMs: 3000, radius: 2.5 },
};
/** Same colours as the bus view: hits from another group orange, forwarded ones yellow. */
const HIT_CROSS = 0xd95926;
const HIT_FORWARDED = 0xc98500;

/** Grid position in tiles (fractions allowed) -> screen position. Tile (x, y)'s top corner is iso(x, y). */
function iso(x: number, y: number) {
  return { x: (x - y) * (TILE_W / 2), y: (x + y) * (TILE_H / 2) };
}

/** Screen offset -> grid direction (inverse of iso). */
function screenToGrid(sx: number, sy: number) {
  const a = sx / (TILE_W / 2);
  const b = sy / (TILE_H / 2);
  return { x: (a + b) / 2, y: (b - a) / 2 };
}

function diamond(cx: number, cy: number): number[] {
  return [cx, cy, cx + TILE_W / 2, cy + TILE_H / 2, cx, cy + TILE_H, cx - TILE_W / 2, cy + TILE_H / 2];
}

function drawGround(map: MapData): Graphics {
  const g = new Graphics();
  for (let y = 0; y < map.height; y++) {
    for (let x = 0; x < map.width; x++) {
      const tile = map.tiles[y * map.width + x];
      const c = iso(x, y);
      const color = TILE_COLORS[tile][(x + y) % 2];
      g.poly(diamond(c.x, c.y)).fill(color).stroke({ width: 1, color: 0x000000, alpha: 0.12 });
    }
  }
  return g;
}

function drawWall(x: number, y: number): Graphics {
  const c = iso(x, y);
  const g = new Graphics();
  // Left face, right face, then the raised top.
  g.poly([c.x - TILE_W / 2, c.y + TILE_H / 2 - WALL_H, c.x, c.y + TILE_H - WALL_H, c.x, c.y + TILE_H, c.x - TILE_W / 2, c.y + TILE_H / 2]).fill(0x67676e);
  g.poly([c.x, c.y + TILE_H - WALL_H, c.x + TILE_W / 2, c.y + TILE_H / 2 - WALL_H, c.x + TILE_W / 2, c.y + TILE_H / 2, c.x, c.y + TILE_H]).fill(0x52525a);
  g.poly(diamond(c.x, c.y - WALL_H)).fill(0x9a9aa2).stroke({ width: 1, color: 0x000000, alpha: 0.25 });
  g.zIndex = x + y + 1; // depth of the tile's centre, compared against units' x + y
  return g;
}

/** A shape baked into a texture, and where its top-left corner sits relative to the unit's feet. */
type Part = { tex: Texture; x: number; y: number };

/** Shapes every unit shares, drawn once into textures. Sprites made from a few shared
 *  textures batch into a handful of draw calls and cost 4 vertices each; vector shapes cost
 *  hundreds of vertices each and are re-sent to the GPU whenever anything moves or re-sorts. */
let PARTS: { shadow: Part; body: Part; head: Part; botHead: Part; ring: Part } | undefined;
const RES = Math.max(2, Math.ceil(window.devicePixelRatio || 1));

function bakeParts(renderer: Renderer) {
  const bake = (g: Graphics, x: number, y: number, w: number, h: number): Part => {
    const tex = renderer.generateTexture({ target: g, frame: new Rectangle(x, y, w, h), resolution: RES, antialias: true });
    g.destroy();
    return { tex, x, y };
  };
  PARTS = {
    shadow: bake(new Graphics().ellipse(0, 0, 14, 7).fill({ color: 0x000000, alpha: 0.3 }), -15, -8, 30, 16),
    // White, so a tint gives each unit its colour; the dark outline stays dark.
    body: bake(new Graphics().roundRect(-9, -34, 18, 32, 8).fill(0xffffff).stroke({ width: 1.5, color: 0x111111 }), -10, -35, 20, 34),
    head: bake(new Graphics().circle(0, -42, 8).fill(0xf1d3b3).stroke({ width: 1.5, color: 0x111111 }), -9, -51, 18, 18),
    // Bots get a square grey head.
    botHead: bake(new Graphics().roundRect(-7, -49, 14, 14, 3).fill(0xb8c0cc).stroke({ width: 1.5, color: 0x111111 }), -8, -50, 16, 16),
    // White, tinted with the Server Entity Group's colour.
    ring: bake(new Graphics().ellipse(0, 0, 19, 10).stroke({ width: 3, color: 0xffffff }), -21, -12, 42, 24),
  };
  // Text drawn from shared glyph atlases (one texture each), so thousands of names and damage
  // numbers batch with the sprites instead of each being its own texture. White, so tint colours it.
  const chars = [[' ', '~'], '→·'];
  const stroke = { color: 0x000000, width: 3 };
  BitmapFont.install({ name: 'label', style: { fontFamily: 'system-ui, sans-serif', fontSize: 12, fill: 0xffffff, stroke }, chars, resolution: RES });
  BitmapFont.install({ name: 'label-bold', style: { fontFamily: 'system-ui, sans-serif', fontSize: 13, fontWeight: '700', fill: 0xffffff, stroke }, chars, resolution: RES });
}

function partSprite(part: Part): Sprite {
  const s = new Sprite(part.tex);
  s.position.set(part.x, part.y);
  return s;
}

/** Text from one of the shared fonts ('label' 12 px, 'label-bold' 13 px), coloured by tint. */
function label(text: string, font: 'label' | 'label-bold', size?: number, tint = 0xffffff): BitmapText {
  const t = new BitmapText({ text, style: { fontFamily: font, fontSize: size ?? (font === 'label' ? 12 : 13) } });
  t.tint = tint;
  return t;
}

function drawUnit(id: number, bot: boolean, isMe: boolean) {
  const parts = PARTS!;
  const sprite = new Container();
  const ring = partSprite(parts.ring); // Server Entity Group colour, shown with the debug overlay
  const shadow = partSprite(parts.shadow);
  // Your own body keeps its thick white outline (one unit, so a vector shape is fine).
  const body = isMe
    ? new Graphics().roundRect(-9, -34, 18, 32, 8).fill(colorOf(id)).stroke({ width: 3, color: 0xffffff })
    : Object.assign(partSprite(parts.body), { tint: colorOf(id) });
  const head = partSprite(bot ? parts.botHead : parts.head);
  const hpBack = new Sprite(Texture.WHITE);
  hpBack.tint = 0x000000;
  hpBack.alpha = 0.6;
  hpBack.position.set(-14, -57);
  hpBack.setSize(28, 4);
  const hpFill = new Sprite(Texture.WHITE);
  hpFill.position.set(-14, -57);
  hpFill.setSize(28, 4);
  const name = label('', 'label');
  name.anchor.set(0.5, 1);
  name.y = -59;
  sprite.addChild(ring, shadow, body, head, hpBack, hpFill, name);
  return { sprite, ring, hpFill, label: name };
}

function drawHp(fill: Sprite, hp: number) {
  fill.tint = hp > 50 ? 0x5ac85a : hp > 25 ? 0xe0b030 : 0xe05050;
  fill.visible = hp > 0;
  fill.setSize((28 * Math.max(0, hp)) / MAX_HP, 4);
}

/** Translucent outline of a unit at its raw, latest server position. */
function drawGhost(isMe: boolean): Container {
  const ghost = new Container();
  const shape = new Graphics()
    .ellipse(0, 0, 14, 7)
    .stroke({ width: 2, color: 0xffffff, alpha: 0.8 })
    .roundRect(-9, -34, 18, 32, 8)
    .fill({ color: 0xffffff, alpha: 0.12 })
    .stroke({ width: 2, color: 0xffffff, alpha: 0.8 })
    .circle(0, -42, 8)
    .fill({ color: 0xffffff, alpha: 0.12 })
    .stroke({ width: 2, color: 0xffffff, alpha: 0.8 });
  ghost.addChild(shape);
  if (isMe) {
    const label = new Text({
      text: 'server',
      style: { fontFamily: 'ui-monospace, monospace', fontSize: 11, fill: 0xffffff, stroke: { color: 0x000000, width: 3 } },
    });
    label.anchor.set(0.5, 0);
    label.y = 8;
    ghost.addChild(label);
  }
  ghost.alpha = 0.85;
  return ghost;
}

/** A unit as the client knows it, flattened for drawing. */
type Unit = Look & { id: number; name: string; x: number; y: number; group: number; spatial: number };

/** A unit on screen. Its positions live in the client (`samples` is the client's array). */
type Drawn = {
  sprite: Container;
  ring: Sprite;
  hpFill: Sprite;
  label: BitmapText;
  state: Unit;
  /** Grid position drawn this frame. */
  drawn: { x: number; y: number };
  /** Raw server position marker (debug; only exists while that overlay is on). */
  ghost?: Container;
  ringColor: number;
  group: number;
  samples: { t: number; x: number; y: number }[];
};

function make<K extends keyof HTMLElementTagNameMap>(tag: K, cls: string, parent: HTMLElement = document.body): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  el.className = cls;
  parent.appendChild(el);
  return el;
}

async function main() {
  const app = new Application();
  await app.init({ background: '#14171c', resizeTo: window, antialias: new URLSearchParams(location.search).get('aa') !== '0' });
  Object.assign(window, { __app: app }); // for automated tests (scene size)
  document.body.prepend(app.canvas);

  bakeParts(app.renderer);
  // Render groups: moving the camera (world) only changes one transform on the GPU, and the
  // ground (4,096 tiles, never changes) is never re-sent when units move or re-sort.
  const world = new Container({ isRenderGroup: true });
  const groundLayer = new Container({ isRenderGroup: true });
  const spatialLayer = new Container(); // debug: Spatial Layer Cell areas
  const senseLayer = new Graphics(); // debug: "what's near me?" circles, on the ground
  const viewLayer = new Graphics(); // debug: the outline of your view
  const markLayer = new Graphics(); // target ring and weapon range, on the ground
  const entityLayer = new Container({ isRenderGroup: true }); // walls + units + markers, depth sorted by x + y
  entityLayer.sortableChildren = true;
  const fxLayer = new Container(); // arrows, slashes, damage numbers
  const fxLines = new Graphics();
  fxLayer.addChild(fxLines);
  const labelLayer = new Container(); // debug: Spatial Layer Cell names, above everything
  world.addChild(groundLayer, spatialLayer, senseLayer, markLayer, entityLayer, fxLayer, viewLayer, labelLayer);

  const hud = new Text({
    text: '',
    style: { fontFamily: 'ui-monospace, monospace', fontSize: 13, fill: 0xe8e8e8, lineHeight: 18 },
  });
  hud.position.set(12, 10);
  app.stage.addChild(world, hud);

  let grid: Grid | undefined;
  let playerCount = 0;
  const units = new Map<number, Drawn>();
  const rendered = { x: 0, y: 0, group: 0, spatial: 0 };
  const latestServerMe = { x: 0, y: 0 };

  // ---- Combat state ----
  let myTarget: number | null = null;
  let myWeapon: 'sword' | 'bow' = 'bow';
  let meDead = false;
  let outOfRange = false;
  /** When each ability may be asked for again (client-side guess; the server enforces it). */
  const readyAt: Record<Ability, number> = { sword: 0, bow: 0, fireball: 0 };
  /** Effects waiting for (or playing at) their moment on the render clock. */
  type Fx = { at: number; ev: Happening; text?: BitmapText; note?: BitmapText };
  let effects: Fx[] = [];
  /** Effects seen, per second, for the last 10 s: [second, within one group, across, forwarded, total delay across]. */
  const hitBuckets: [number, number, number, number, number][] = [];

  // ---- Debug overlay state ----
  const spatialLabels = new Map<number, Text>();
  /** Questions asked, drawn for a moment after they're asked. */
  let senseMarks: { at: number; s: SenseDebug }[] = [];
  /** Units that just moved to another group: a ring that grows and a note, for a second. */
  let handoffMarks: { at: number; id: number; from: number; to: number; note?: BitmapText }[] = [];

  const client = new WorldmeshClient<Components, Events, Welcome, Move, Commands, MoveState>(
    {
      url: GATEWAY_URL,
      prediction: movementRules(() => grid),
      // We don't move while dead: show the server's view.
      shouldPredict: (own) => own.view.dead === undefined,
    },
    {
      status: (status) => {
        if (status === 'connected') wantDir = { x: 0, y: 0 };
        updateHud();
      },
      welcome: (w) => {
        setTickMs(w.tick_ms);
        loadMap(w.game.map);
      },
      snapshot: (s) => applySnapshot(s),
      removed: (id) => {
        const e = units.get(id);
        e?.sprite.destroy({ children: true });
        e?.ghost?.destroy({ children: true });
        units.delete(id);
      },
      handoff: (id, from, to) => {
        handoffMarks = handoffMarks.filter((m) => m.id !== id || (m.note?.destroy(), false));
        handoffMarks.push({ at: performance.now(), id, from, to });
      },
      ownership: () => drawSpatial(),
      stats: () => updateSpatialLabels(),
    },
  );

  Object.assign(window, { __client: client }); // for automated tests and poking around in the console

  const OUTCOME_SHORT = { hit: 'hit', killed: 'killed', out_of_range: 'out of range', blocked: 'blocked' };
  const panel = new DebugPanel({
    client,
    onOverlay: (overlay) => {
      if (overlay === 'groups') for (const e of units.values()) updateRing(e);
      if (overlay === 'spatial') drawSpatial();
    },
    // For the bus view's "recent actions": who did what, and how it ended.
    traceNote: (event): TraceNote | undefined => {
      const ev = { k: event.name, ...(event.data as object) } as Happening;
      if (ev.k === 'used') {
        const aim = 'entity' in ev.target ? ` → ${unitName(ev.target.entity)}` : '';
        return { trace: ev.trace, start: { label: `${unitName(ev.actor)} ${ev.ability}${aim}`, group: ev.group, area: !('entity' in ev.target) } };
      }
      const landed = ev.outcome === 'hit' || ev.outcome === 'killed';
      return { trace: ev.trace, result: { text: ev.outcome === 'hit' ? `hit ${ev.damage}` : OUTCOME_SHORT[ev.outcome], landed } };
    },
  });
  const short = (id: number) => panel.short(id);
  const groupColor = (id: number) => panel.groupColor(id);
  const css = (color: number) => `#${color.toString(16).padStart(6, '0')}`;
  const tickMs = () => client.tickMs;

  window.gameDebug = {
    ...rendered,
    predicted: { x: 0, y: 0 },
    server: latestServerMe,
    predTick: -1,
    corrections: 0,
    lead: 0,
  };

  const updateHud = () => {
    const p = client.predictor;
    hud.text = [
      `${client.status} to ${GATEWAY_URL}`,
      `tick ${client.lastTick}   ${playerCount} in view   you (${rendered.x.toFixed(2)}, ${rendered.y.toFixed(2)})`,
      `simulated by ${rendered.group ? panel.groupName(rendered.group) : '-'} · standing in ${rendered.spatial ? panel.spatialName(rendered.spatial) : '-'}`,
      `prediction ${p?.enabled ? 'on' : 'off'}   ${Math.round(1000 / tickMs())} ticks/s (${tickMs()} ms)`,
      'WASD / arrows, or hold the left mouse button to walk toward the cursor.',
      `Right-click: target · Q: ${myWeapon} (${ABILITIES[myWeapon].range} tiles) · F: fireball at the cursor · Esc: stop` +
        (myTarget !== null ? `   target: ${units.get(myTarget)?.state.name ?? myTarget}${outOfRange ? ' (out of range)' : ''}` : ''),
      meDead ? 'You died. Respawning in town...' : '',
    ].join('\n');
  };

  // ---- Debug overlays ----
  const updateRing = (e: Drawn) => {
    const color = groupColor(e.group);
    if (e.ringColor !== color) {
      e.ring.tint = color;
      e.ringColor = color;
    }
    e.ring.visible = panel.overlays.groups;
  };

  const updateSpatialLabels = () => {
    for (const [id, label] of spatialLabels) {
      const s = panel.processStats.get(id);
      const text = `${short(id)}${s ? `\n${s.players} held · ${s.viewers} watching` : ''}`;
      if (label.text !== text) label.text = text;
    }
  };

  /** Shade every chunk by the Spatial Layer Cell owning it, outline the areas, label each one. */
  const drawSpatial = () => {
    spatialLayer.removeChildren().forEach((c) => c.destroy());
    labelLayer.removeChildren().forEach((c) => c.destroy());
    spatialLabels.clear();
    for (const e of units.values()) updateRing(e);
    const assignment: Assignment | undefined = client.assignment;
    if (!assignment || !panel.overlays.spatial) return;

    const { chunk_size: cs, chunks_x: nx, chunks_y: ny, owners } = assignment;
    const fill = new Graphics();
    const gridLines = new Graphics();
    const borders = new Graphics();
    for (let cy = 0; cy < ny; cy++) {
      for (let cx = 0; cx < nx; cx++) {
        const owner = owners[cy * nx + cx];
        const [x0, y0, x1, y1] = [cx * cs, cy * cs, (cx + 1) * cs, (cy + 1) * cs];
        const corners = [iso(x0, y0), iso(x1, y0), iso(x1, y1), iso(x0, y1)].flatMap((p) => [p.x, p.y]);
        // Areas are told apart by shade and borders; colour is kept for groups.
        const i = panel.spatialIndex(owner);
        fill.poly(corners).fill(i < 0 ? { color: 0x000000, alpha: 0.35 } : { color: i % 2 ? 0x000000 : 0xffffff, alpha: i % 2 ? 0.16 + 0.06 * (i % 3) : 0.1 + 0.05 * (i % 3) });
        if (cx + 1 < nx && owners[cy * nx + cx + 1] !== owner) {
          const [a, b] = [iso(x1, y0), iso(x1, y1)];
          borders.moveTo(a.x, a.y).lineTo(b.x, b.y);
        }
        if (cy + 1 < ny && owners[(cy + 1) * nx + cx] !== owner) {
          const [a, b] = [iso(x0, y1), iso(x1, y1)];
          borders.moveTo(a.x, a.y).lineTo(b.x, b.y);
        }
      }
    }
    // Faint chunk grid so the chunk size is visible.
    for (let cx = 1; cx < nx; cx++) {
      const [a, b] = [iso(cx * cs, 0), iso(cx * cs, ny * cs)];
      gridLines.moveTo(a.x, a.y).lineTo(b.x, b.y);
    }
    for (let cy = 1; cy < ny; cy++) {
      const [a, b] = [iso(0, cy * cs), iso(nx * cs, cy * cs)];
      gridLines.moveTo(a.x, a.y).lineTo(b.x, b.y);
    }
    gridLines.stroke({ width: 1, color: 0xffffff, alpha: 0.15 });
    borders.stroke({ width: 3, color: 0xffffff, alpha: 0.85 });
    spatialLayer.addChild(fill, gridLines, borders);

    for (const id of assignment.spatial) {
      let sx = 0, sy = 0, n = 0;
      owners.forEach((owner, i) => {
        if (owner !== id) return;
        sx += ((i % nx) + 0.5) * cs;
        sy += (Math.floor(i / nx) + 0.5) * cs;
        n++;
      });
      if (!n) continue;
      const c = iso(sx / n, sy / n);
      const label = new Text({
        text: short(id),
        style: {
          fontFamily: 'system-ui, sans-serif', fontSize: 16, fontWeight: '700', align: 'center',
          fill: 0xf0efec, stroke: { color: 0x000000, width: 4 },
        },
      });
      label.anchor.set(0.5);
      label.position.set(c.x, c.y);
      label.alpha = 0.9;
      labelLayer.addChild(label);
      spatialLabels.set(id, label);
    }
    updateSpatialLabels();
  };

  /** Questions (circles, thicker when they crossed into a second cell's area) and handoffs
   *  (a ring growing in the new group's colour, with a note). */
  const drawDebugMarks = () => {
    const now = performance.now();
    senseLayer.clear();
    senseMarks = senseMarks.filter((m) => now - m.at < 300);
    if (panel.overlays.senses) {
      // Thin outlines that fade quickly: dozens of bots ask several times a second.
      for (const { at, s } of senseMarks) {
        const k = (now - at) / 300;
        const c = iso(s.x, s.y);
        const r = s.r * Math.SQRT2; // a circle of r tiles is an ellipse on screen
        senseLayer
          .ellipse(c.x, c.y, (r * TILE_W) / 2, (r * TILE_H) / 2)
          .stroke({ width: s.spatial.length > 1 ? 2.5 : 1, color: SENSE_COLOR, alpha: 0.7 * (1 - k) });
      }
    }
    handoffMarks = handoffMarks.filter((m) => {
      const age = now - m.at;
      const e = units.get(m.id);
      if (age > 1200 || !e) {
        m.note?.destroy();
        return false;
      }
      const k = age / 1200;
      const { x, y } = e.sprite.position;
      fxLines.ellipse(x, y, 19 + 30 * k, 10 + 15 * k).stroke({ width: 3, color: groupColor(m.to), alpha: 1 - k });
      if (!m.note) {
        m.note = label(`${short(m.from)} → ${short(m.to)}`, 'label-bold', 12, groupColor(m.to));
        m.note.anchor.set(0.5);
        fxLayer.addChild(m.note);
      }
      m.note.position.set(x, y + 22);
      m.note.alpha = 1 - k * k;
      return true;
    });
    drawMinimap();
    viewLayer.clear();
    const me = units.get(client.me);
    const view = client.world?.view;
    if (panel.overlays.view && me && view) {
      // The view is a screen-sized rectangle around you (|dx - dy| <= w, |dx + dy| <= h).
      const { x, y } = me.sprite.position;
      const hw = view.half_w * (TILE_W / 2);
      const hh = view.half_h * (TILE_H / 2);
      viewLayer.rect(x - hw, y - hh, hw * 2, hh * 2).stroke({ width: 3, color: 0xf0efec, alpha: 0.8 });
    }
  };

  // ---- Minimap: the whole world, the Spatial Layer Cells' areas, your view and what's in it ----
  const minimap = panel.minimap;
  let minimapAt = 0;
  const drawMinimap = () => {
    const now = performance.now();
    const assignment = client.assignment;
    const view = client.world?.view;
    if (!panel.overlays.view || !assignment || !grid || !view || now - minimapAt < 100) return;
    minimapAt = now;
    const W = 226;
    const k = W / (grid.width + grid.height) / (TILE_W / 2); // whole map width on screen -> W px
    const H = Math.ceil((grid.width + grid.height) * (TILE_H / 2) * k) + 4;
    const dpr = window.devicePixelRatio || 1;
    if (minimap.width !== W * dpr) {
      minimap.width = W * dpr;
      minimap.height = H * dpr;
      minimap.style.width = `${W}px`;
      minimap.style.height = `${H}px`;
    }
    const ctx = minimap.getContext('2d')!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    const at = (x: number, y: number) => {
      const p = iso(x, y);
      return { x: p.x * k + grid!.height * (TILE_W / 2) * k, y: p.y * k + 2 };
    };
    const { chunk_size: cs, chunks_x: nx, chunks_y: ny, owners } = assignment;
    for (let cy = 0; cy < ny; cy++) {
      for (let cx = 0; cx < nx; cx++) {
        const i = panel.spatialIndex(owners[cy * nx + cx]);
        const pts = [at(cx * cs, cy * cs), at((cx + 1) * cs, cy * cs), at((cx + 1) * cs, (cy + 1) * cs), at(cx * cs, (cy + 1) * cs)];
        ctx.beginPath();
        pts.forEach((p, j) => (j ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y)));
        ctx.closePath();
        ctx.fillStyle = i < 0 ? '#1a1a1a' : ['#3b4250', '#262b33', '#4a5262', '#1f232a'][i % 4];
        ctx.fill();
      }
    }
    // Borders between areas, and each area's letter.
    ctx.strokeStyle = '#c3c2b7';
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    for (let cy = 0; cy < ny; cy++) {
      for (let cx = 0; cx < nx; cx++) {
        const o = owners[cy * nx + cx];
        if (cx + 1 < nx && owners[cy * nx + cx + 1] !== o) {
          const [a, b] = [at((cx + 1) * cs, cy * cs), at((cx + 1) * cs, (cy + 1) * cs)];
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
        }
        if (cy + 1 < ny && owners[(cy + 1) * nx + cx] !== o) {
          const [a, b] = [at(cx * cs, (cy + 1) * cs), at((cx + 1) * cs, (cy + 1) * cs)];
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
        }
      }
    }
    ctx.stroke();
    ctx.font = '600 11px system-ui, sans-serif';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillStyle = '#f0efec';
    assignment.spatial.forEach((id) => {
      let sx = 0, sy = 0, n = 0;
      owners.forEach((o, i) => {
        if (o !== id) return;
        sx += ((i % nx) + 0.5) * cs;
        sy += (Math.floor(i / nx) + 0.5) * cs;
        n++;
      });
      if (n) {
        const c = at(sx / n, sy / n);
        ctx.fillText(short(id).replace('Spatial ', ''), c.x, c.y);
      }
    });
    // What you receive: every unit in your view, by group.
    for (const e of units.values()) {
      const p = at(e.drawn.x, e.drawn.y);
      ctx.beginPath();
      ctx.arc(p.x, p.y, e.state.id === client.me ? 3 : 2, 0, Math.PI * 2);
      ctx.fillStyle = e.state.id === client.me ? '#ffffff' : css(groupColor(e.group));
      ctx.fill();
    }
    // Your view: a screen-sized rectangle around you.
    const me = units.get(client.me);
    if (me) {
      const c = at(me.drawn.x, me.drawn.y);
      const hw = view.half_w * (TILE_W / 2) * k;
      const hh = view.half_h * (TILE_H / 2) * k;
      ctx.strokeStyle = '#ffffff';
      ctx.lineWidth = 1.5;
      ctx.strokeRect(c.x - hw, c.y - hh, hw * 2, hh * 2);
    }
  };

  // ---- What the server sends ----
  const loadMap = (map: MapData) => {
    grid = map;
    groundLayer.removeChildren().forEach((c) => c.destroy());
    entityLayer.removeChildren().forEach((c) => c.destroy({ children: true }));
    units.clear();
    groundLayer.addChild(drawGround(map));
    for (let y = 0; y < map.height; y++) {
      for (let x = 0; x < map.width; x++) {
        if (map.tiles[y * map.width + x] === WALL) entityLayer.addChild(drawWall(x, y));
      }
    }
  };

  const updateLabel = (e: Drawn) => {
    const s = e.state;
    const text = `${s.name}${s.id === client.me ? ' (you)' : ''}${s.dead ? ' (dead)' : ''}`;
    if (e.label.text !== text) e.label.text = text;
    e.sprite.alpha = s.dead ? 0.35 : 1;
  };

  const logEl = make('div', 'combat-log');
  /** When each line was added; one timer fades and removes them. */
  const logTimes = new WeakMap<Element, number>();
  setInterval(() => {
    const now = performance.now();
    for (const line of [...logEl.children]) {
      const age = now - (logTimes.get(line) ?? now);
      if (age > 6000) line.remove();
      else if (age > 5000) line.classList.add('fade');
    }
  }, 250);
  /** Lines about other units' fights, at most a few per second (a big fight has hundreds
   *  per second, and only 7 lines show); lines about you always go in. */
  let othersLogged = { second: 0, count: 0 };
  const log = (text: string, mine = true) => {
    if (!mine) {
      const second = Math.floor(performance.now() / 1000);
      if (othersLogged.second !== second) othersLogged = { second, count: 0 };
      if (++othersLogged.count > 3) return;
    }
    const line = document.createElement('div');
    line.textContent = text;
    logTimes.set(line, performance.now());
    logEl.prepend(line);
    while (logEl.children.length > 7) logEl.lastElementChild!.remove();
  };

  const unitName = (id: number) => units.get(id)?.state.name ?? nameOf(id, false);

  const OUTCOME_TEXT = { hit: 'hit', killed: 'killed', out_of_range: 'missed (out of reach)', blocked: 'missed (a wall in the way)' };

  const onGameEvent = (ev: Happening) => {
    effects.push({ at: ev.tick * tickMs(), ev });
    if (ev.k !== 'effect') return;
    const cross = ev.from_group !== ev.to_group;
    const second = Math.floor(performance.now() / 1000);
    let bucket = hitBuckets[hitBuckets.length - 1];
    if (!bucket || bucket[0] !== second) hitBuckets.push((bucket = [second, 0, 0, 0, 0]));
    if (cross) {
      bucket[2]++;
      bucket[4] += ev.delay_ticks;
    } else bucket[1]++;
    if (ev.forwarded) bucket[3]++;
    const mine = ev.actor === client.me || ev.target === client.me;
    if (!cross && !mine) return;
    const where = cross
      ? ` · ${short(ev.from_group)} → ${short(ev.to_group)}${ev.forwarded ? ' (forwarded after a handoff)' : ''}`
      : ` · within ${short(ev.to_group)}`;
    const amount = ev.damage ? ` ${ev.damage}` : '';
    log(`${unitName(ev.actor)} ${OUTCOME_TEXT[ev.outcome]} ${unitName(ev.target)} (${ev.ability}${amount})${where}`, mine);
  };
  client.on('used', (used) => onGameEvent({ k: 'used', ...used }));
  client.on('effect', (effect) => onGameEvent({ k: 'effect', ...effect }));

  // ---- Combat and bots (a section of the debug panel) ----
  const bots = panel.addSection('Combat and bots');
  bots.innerHTML =
    `<div class="row"><input type="number" class="bots-count" value="20" min="1" max="200" /> bots ` +
    `<button class="wide bots-spawn">Spawn</button><button class="wide bots-clear">Clear</button></div>` +
    `<div class="row">New bots <select class="bots-mode" title="How the next bots you spawn behave">` +
    `<option value="aggressive" selected>pick fights</option><option value="fight_back">only fight back</option>` +
    `<option value="wander">just run around</option></select></div><div class="muted combat-stats"></div>`;
  const q = <T extends HTMLElement>(cls: string) => bots.querySelector<T>(`.${cls}`)!;
  q('bots-spawn').addEventListener('click', (e) => {
    const count = Math.max(1, Math.min(200, Number(q<HTMLInputElement>('bots-count').value) || 20));
    client.sendCommand('spawn_bots', { count, mode: q<HTMLSelectElement>('bots-mode').value as BotMode });
    (e.currentTarget as HTMLElement).blur();
  });
  q('bots-clear').addEventListener('click', (e) => {
    client.sendCommand('clear_bots', null);
    (e.currentTarget as HTMLElement).blur();
  });
  const combatStats = q('combat-stats');
  combatStats.style.marginTop = '4px';
  combatStats.style.maxWidth = '230px';

  const updateCombatStats = () => {
    const now = performance.now();
    const since = Math.floor(now / 1000) - 9;
    while (hitBuckets.length && hitBuckets[0][0] < since) hitBuckets.shift();
    let within = 0, across = 0, fwd = 0, delay = 0;
    for (const [, w, c, f, d] of hitBuckets) {
      within += w;
      across += c;
      fwd += f;
      delay += d;
    }
    const avg = across ? delay / across : 0;
    const text =
      `last 10 s: ${within} hits within one group, ${across} across groups ` +
      `(all land +${avg ? avg.toFixed(1) : '1'} tick later), ${fwd} forwarded after a handoff`;
    if (combatStats.textContent !== text) combatStats.textContent = text;
  };

  const toUnit = (s: SnapshotEntity<Components>): Unit => ({
    ...lookOf(s),
    id: s.id,
    name: nameOf(s.id, s.kind === 'bot'),
    x: s.x,
    y: s.y,
    group: s.group,
    spatial: s.spatial,
  });

  const applySnapshot = (msg: { entities: SnapshotEntity<Components>[]; senses: SenseDebug[] }) => {
    for (const s of msg.entities) {
      const unit = toUnit(s);
      let e = units.get(s.id);
      if (!e) {
        const isMe = s.id === client.me;
        const { sprite, ring, hpFill, label } = drawUnit(s.id, unit.bot, isMe);
        const tracked = client.entities.get(s.id)!;
        e = { sprite, ring, hpFill, label, state: { ...unit, hp: -1 }, drawn: { x: s.x, y: s.y }, ringColor: -1, group: s.group, samples: tracked.samples };
        units.set(s.id, e);
        entityLayer.addChild(sprite);
      }
      e.group = s.group;
      if (e.state.hp !== unit.hp) drawHp(e.hpFill, unit.hp);
      e.state = unit;
      updateLabel(e);
      updateRing(e);
      if (s.id === client.me) {
        rendered.group = s.group;
        rendered.spatial = s.spatial;
        latestServerMe.x = s.x;
        latestServerMe.y = s.y;
        meDead = unit.dead;
      }
    }
    const now = performance.now();
    for (const sense of msg.senses) senseMarks.push({ at: now, s: sense });
    updateCombatStats();
    playerCount = msg.entities.length;
    updateHud();
  };

  /** Screen position of a unit's torso as drawn this frame. */
  const torso = (id: number) => {
    const e = units.get(id);
    return e && { x: e.sprite.position.x, y: e.sprite.position.y - 22 };
  };

  /** Screen position of a grid point on the ground. */
  const ground = (x: number, y: number) => iso(x, y);

  /** Target ring, weapon range, and each use and effect played at its moment on the render clock. */
  const drawCombat = (renderTime: number) => {
    markLayer.clear();
    const me = units.get(client.me);
    const target = myTarget !== null ? units.get(myTarget) : undefined;
    if (me && target) {
      // A circle of `range` tiles on the grid is an ellipse on screen.
      const r = ABILITIES[myWeapon].range * Math.SQRT2;
      markLayer
        .ellipse(me.sprite.position.x, me.sprite.position.y, (r * TILE_W) / 2, (r * TILE_H) / 2)
        .stroke({ width: 1.5, color: 0xffffff, alpha: outOfRange ? 0.6 : 0.3 })
        .ellipse(target.sprite.position.x, target.sprite.position.y, 20, 10)
        .stroke({ width: 3, color: outOfRange ? 0x999999 : 0xe05050 });
    }

    fxLines.clear();
    const tick = tickMs();
    effects = effects.filter((fx) => {
      const age = renderTime - fx.at;
      if (age < 0) return true;
      const { ev } = fx;
      if (age > 900) {
        fx.text?.destroy();
        fx.note?.destroy();
        return false;
      }
      if (ev.k === 'used') {
        // The use: an arrow or fireball flies for exactly one tick, because its hit lands on
        // the next tick wherever the target is.
        const from = torso(ev.actor) ?? { x: ground(ev.x, ev.y).x, y: ground(ev.x, ev.y).y - 22 };
        const k = Math.min(1, age / tick);
        if ('entity' in ev.target) {
          const to = torso(ev.target.entity);
          if (!to) return age < tick;
          if (ev.ability === 'bow' && k < 1) {
            const hx = from.x + (to.x - from.x) * k;
            const hy = from.y + (to.y - from.y) * k;
            const len = Math.hypot(to.x - from.x, to.y - from.y) || 1;
            const [ux, uy] = [(to.x - from.x) / len, (to.y - from.y) / len];
            fxLines.moveTo(hx - ux * 16, hy - uy * 16).lineTo(hx, hy).stroke({ width: 3, color: 0xffffff });
          }
          if (ev.ability === 'sword' && age < 180) {
            const a0 = -1.1 + age / 120;
            fxLines.moveTo(to.x + Math.cos(a0) * 18, to.y + Math.sin(a0) * 18).arc(to.x, to.y, 18, a0, a0 + 1.7).stroke({ width: 3, color: 0xffffff });
          }
          return age < tick + 200;
        }
        // Fireball: an orb to the point, then the blast (radius from the ability table).
        const [px, py] = ev.target.point;
        const at = ground(px, py);
        if (k < 1) {
          fxLines.circle(from.x + (at.x - from.x) * k, from.y - 10 + (at.y - from.y) * k, 6).fill(0xff7a2f);
        } else {
          const blast = Math.min(1, (age - tick) / 250);
          const r = ABILITIES.fireball.radius * Math.SQRT2 * blast;
          fxLines
            .ellipse(at.x, at.y, (r * TILE_W) / 2, (r * TILE_H) / 2)
            .fill({ color: 0xff7a2f, alpha: 0.35 * (1 - blast) + 0.1 })
            .stroke({ width: 2, color: 0xff7a2f, alpha: 1 - blast * 0.6 });
        }
        return age < tick + 300;
      }

      // The effect, as the target's group decided it.
      const to = torso(ev.target);
      const from = torso(ev.actor);
      if (!to) return false;
      const color = ev.forwarded ? HIT_FORWARDED : ev.from_group !== ev.to_group ? HIT_CROSS : 0xffffff;
      if (!fx.text) {
        const miss = ev.outcome === 'out_of_range' || ev.outcome === 'blocked';
        fx.text = label(
          ev.outcome === 'killed' ? 'KO' : miss ? (ev.outcome === 'blocked' ? 'blocked' : 'miss') : `-${ev.damage}`,
          'label-bold',
          ev.outcome === 'killed' ? 16 : 13,
          ev.outcome === 'killed' ? 0xff6b6b : miss ? 0xbbbbbb : color,
        );
        fx.text.anchor.set(0.5);
        fxLayer.addChild(fx.text);
        if (panel.overlays.groups && ev.from_group !== ev.to_group) {
          fx.note = label(`${short(ev.from_group)} → ${short(ev.to_group)}${ev.forwarded ? ' (forwarded)' : ''}`, 'label', 11, color);
          fx.note.anchor.set(0.5);
          fxLayer.addChild(fx.note);
        }
      }
      const rise = Math.min(1, age / 900);
      fx.text.position.set(to.x, to.y - 40 - rise * 26);
      fx.text.alpha = 1 - rise * rise;
      if (fx.note) {
        const mid = from ? { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 - 12 } : { x: to.x, y: to.y - 70 };
        fx.note.position.set(mid.x, mid.y);
        fx.note.alpha = 1 - rise * rise;
      }
      return true;
    });
  };

  /** Ask for an ability when the client thinks it can work; the servers decide. */
  const useAbility = (ability: Ability, target: Target) => {
    if (meDead || performance.now() < readyAt[ability]) return false;
    readyAt[ability] = performance.now() + ABILITIES[ability].cooldownMs + 60;
    client.sendCommand('use', { ability, target });
    return true;
  };

  /** Keep using the weapon on the target while it's in reach (as far as this client can see). */
  const autoUse = () => {
    const me = units.get(client.me);
    const target = myTarget !== null ? units.get(myTarget) : undefined;
    if (myTarget !== null && (!target || target.state.dead)) {
      myTarget = null;
      updateHud();
    }
    if (!me || !target) return;
    const dist = Math.hypot(target.drawn.x - me.drawn.x, target.drawn.y - me.drawn.y);
    const far = dist > ABILITIES[myWeapon].range - 0.15;
    if (far !== outOfRange) {
      outOfRange = far;
      updateHud();
    }
    if (!far) useAbility(myWeapon, { entity: myTarget! });
  };

  // ---- Every frame ----
  // Main-thread time per frame, for the metrics: from before our first callback to after Pixi rendered.
  let frameStart = 0;
  app.ticker.add(() => (frameStart = performance.now()), undefined, UPDATE_PRIORITY.INTERACTION);
  app.ticker.add(() => panel.recordFrame(performance.now() - frameStart), undefined, UPDATE_PRIORITY.UTILITY);

  app.ticker.add((ticker) => {
    if (client.clockOffset === undefined) return;
    client.frame(ticker.deltaMS);
    const renderTime = client.renderTime();
    // Camera first, so units off screen can be skipped.
    const camera = client.positionOf(client.me, renderTime);
    if (camera) {
      const c = iso(camera.x, camera.y);
      world.position.set(app.screen.width / 2 - c.x, app.screen.height / 2 - c.y);
      rendered.x = camera.x;
      rendered.y = camera.y;
    }
    const { width: sw, height: sh } = app.screen;
    const ox = world.position.x;
    const oy = world.position.y;
    for (const [id, e] of units) {
      const p = client.positionOf(id, renderTime);
      if (!p) continue;
      const c = iso(p.x, p.y);
      e.sprite.position.set(c.x, c.y);
      e.drawn.x = p.x;
      e.drawn.y = p.y;
      // Off screen (with room for the name above and the ring below): not drawn, and its depth
      // isn't updated, so it doesn't make the layer re-sort.
      const sx = ox + c.x;
      const sy = oy + c.y;
      const onScreen = sx > -60 && sx < sw + 60 && sy > -20 && sy < sh + 90;
      e.sprite.visible = onScreen;
      if (onScreen) e.sprite.zIndex = p.x + p.y;

      if (panel.overlays.ghosts) {
        if (!e.ghost) {
          e.ghost = drawGhost(id === client.me);
          entityLayer.addChild(e.ghost);
        }
        const raw = e.samples[e.samples.length - 1];
        const g = iso(raw.x, raw.y);
        e.ghost.position.set(g.x, g.y);
        e.ghost.zIndex = raw.x + raw.y + 0.01;
        e.ghost.visible = onScreen;
      } else if (e.ghost) {
        e.ghost.destroy({ children: true });
        e.ghost = undefined;
      }
    }
    autoUse();
    drawCombat(renderTime);
    drawDebugMarks();
    const p = client.predictor;
    const predicted = p?.predicted;
    Object.assign(window.gameDebug!, {
      ...rendered,
      predicted: predicted ? { x: predicted.x, y: predicted.y } : { x: 0, y: 0 },
      server: { ...latestServerMe },
      predTick: p?.tick ?? -1,
      corrections: p?.stats.corrections ?? 0,
      lead: p?.lead ?? 0,
      serverClock: client.serverClock(),
      target: myTarget,
      entities: () =>
        [...units.values()].map((e) => ({
          id: e.state.id, name: e.state.name, bot: e.state.bot, hp: e.state.hp, group: e.state.group, spatial: e.state.spatial, x: e.state.x, y: e.state.y,
          sx: world.position.x + e.sprite.position.x, sy: world.position.y + e.sprite.position.y,
        })),
    });
  });

  // ---- Input ----
  // Send "from tick N, move this way" when the wanted direction changes (mouse aim at most 20/s).
  const held = new Set<string>();
  const MOVE_KEYS = new Set(['KeyW', 'KeyA', 'KeyS', 'KeyD', 'ArrowUp', 'ArrowLeft', 'ArrowDown', 'ArrowRight']);
  let mouseDown = false;
  const mouse = { x: 0, y: 0 };
  let lastSentAt = 0;
  let wantDir = { x: 0, y: 0 };
  const f32 = Math.fround;

  /** Unit direction scaled just under 1 and rounded to f32, so the server's length clamp
   *  never alters it and both sides compute identical moves. */
  const toWire = (x: number, y: number) => {
    if (x === 0 && y === 0) return { x: 0, y: 0 };
    const len = Math.hypot(x, y);
    const [wx, wy] = sanitizeDir(f32((x / len) * 0.9999), f32((y / len) * 0.9999));
    return { x: wx, y: wy };
  };

  const wantedDirection = (): { x: number; y: number } => {
    if (mouseDown) {
      // Our unit is always at the screen centre (the camera follows it).
      const sx = mouse.x - app.screen.width / 2;
      const sy = mouse.y - app.screen.height / 2;
      if (Math.hypot(sx, sy) < 12) return { x: 0, y: 0 };
      const g = screenToGrid(sx, sy);
      return toWire(g.x, g.y);
    }
    let sx = 0, sy = 0;
    if (held.has('KeyW') || held.has('ArrowUp')) sy -= 1;
    if (held.has('KeyS') || held.has('ArrowDown')) sy += 1;
    if (held.has('KeyA') || held.has('ArrowLeft')) sx -= 1;
    if (held.has('KeyD') || held.has('ArrowRight')) sx += 1;
    // Screen direction -> grid direction (screen up is grid (-1, -1) in 2:1 iso).
    return toWire(sx + sy, sy - sx);
  };

  const sendInput = (force: boolean) => {
    if (client.clockOffset === undefined) return;
    const want = wantedDirection();
    // Mouse aim: ignore tiny wobbles.
    if (Math.abs(want.x - wantDir.x) < 0.002 && Math.abs(want.y - wantDir.y) < 0.002 && !(want.x === 0 && want.y === 0 && (wantDir.x !== 0 || wantDir.y !== 0))) return;
    const now = performance.now();
    if (!force && now - lastSentAt < 50) return;
    lastSentAt = now;
    wantDir = want;
    // Predicted at once; the server applies it at exactly the same sub-step.
    client.sendInput({ dx: want.x, dy: want.y });
  };
  app.ticker.add(() => sendInput(false));

  /** The unit drawn under a screen point (its body, not just its feet). */
  const unitAt = (sx: number, sy: number) => {
    let best: number | null = null;
    let bestDist = Infinity;
    for (const [id, e] of units) {
      if (id === client.me) continue;
      const px = world.position.x + e.sprite.position.x;
      const py = world.position.y + e.sprite.position.y;
      if (Math.abs(sx - px) > 16 || sy < py - 52 || sy > py + 8) continue;
      const d = Math.hypot(sx - px, sy - (py - 22));
      if (d < bestDist) [best, bestDist] = [id, d];
    }
    return best;
  };
  /** Is the screen point on our own unit (always drawn at the screen centre)? */
  const unitAtSelf = (sx: number, sy: number) => {
    const px = app.screen.width / 2;
    const py = app.screen.height / 2;
    return Math.abs(sx - px) <= 16 && sy >= py - 52 && sy <= py + 8;
  };
  app.canvas.addEventListener('contextmenu', (e) => e.preventDefault());
  // Hovering a unit: who simulates it, whose area it's in.
  const hoverTip = make('div', 'hover-tip');
  app.canvas.addEventListener('pointermove', (e) => {
    const id = unitAt(e.clientX, e.clientY) ?? (client.me >= 0 && unitAtSelf(e.clientX, e.clientY) ? client.me : null);
    const unit = id !== null ? units.get(id) : undefined;
    if (!unit) {
      hoverTip.style.display = 'none';
      return;
    }
    const s = unit.state;
    const target = s.target !== null ? ` · after ${unitName(s.target)}` : '';
    hoverTip.innerHTML =
      `<b>${s.name}</b>${s.id === client.me ? ' (you)' : ''} · hp ${s.hp}/${MAX_HP}${target}<br>` +
      `simulated by <b style="color:${css(groupColor(s.group))}">${panel.groupName(s.group)}</b><br>` +
      `standing in <b>${panel.spatialName(s.spatial)}</b>'s area`;
    hoverTip.style.display = 'block';
    hoverTip.style.left = `${Math.min(e.clientX + 14, window.innerWidth - hoverTip.offsetWidth - 4)}px`;
    hoverTip.style.top = `${Math.max(4, e.clientY - hoverTip.offsetHeight - 12)}px`;
  });
  app.canvas.addEventListener('pointerleave', () => (hoverTip.style.display = 'none'));
  app.canvas.addEventListener('pointerdown', (e) => {
    if (e.button !== 2) return;
    myTarget = unitAt(e.clientX, e.clientY);
    updateHud();
  });
  /** The ground point under the mouse, in tiles. */
  const cursorGrid = () => screenToGrid(mouse.x - world.position.x, mouse.y - world.position.y);

  window.addEventListener('keydown', (e) => {
    if (e.target instanceof HTMLInputElement) return;
    if (e.repeat && !MOVE_KEYS.has(e.code)) return;
    if (e.code === 'KeyQ') {
      myWeapon = myWeapon === 'bow' ? 'sword' : 'bow';
      return updateHud();
    }
    if (e.code === 'Escape') {
      myTarget = null;
      return updateHud();
    }
    if (e.code === 'KeyF') {
      const at = cursorGrid();
      const me = units.get(client.me);
      if (!me) return;
      if (Math.hypot(at.x - me.drawn.x, at.y - me.drawn.y) > ABILITIES.fireball.range) return log('Too far for a fireball (10 tiles)');
      if (!useAbility('fireball', { point: [at.x, at.y] })) log('Fireball not ready yet');
      return;
    }
    if (!MOVE_KEYS.has(e.code)) return;
    e.preventDefault();
    held.add(e.code);
    sendInput(true);
  });
  window.addEventListener('keyup', (e) => {
    if (!MOVE_KEYS.has(e.code)) return;
    held.delete(e.code);
    sendInput(true);
  });
  app.canvas.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    mouseDown = true;
    mouse.x = e.clientX;
    mouse.y = e.clientY;
    sendInput(true);
  });
  window.addEventListener('pointermove', (e) => {
    mouse.x = e.clientX;
    mouse.y = e.clientY;
  });
  window.addEventListener('pointerup', (e) => {
    if (e.button !== 0) return;
    mouseDown = false;
    sendInput(true);
  });
  window.addEventListener('blur', () => {
    held.clear();
    mouseDown = false;
    sendInput(true);
  });

  updateHud();
  client.connect();
}

main();
