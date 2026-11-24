// Live debug graphs over the last 60 s: CPU, memory, and traffic for every server process
// (from the gateway's once-a-second stats) next to this client's own numbers, plus what the
// Server Entity Groups and Spatial Layer Cells are doing.

import type { ProcessStats, Rate } from '@worldmesh/client';
import { gatewayTotals } from './gateways.ts';

/** Kinds of bus message in the "by kind" graph, in the order the server reports them. */
const BUS_KINDS: { kind: string; name: string; color: string }[] = [
  { kind: 'reports', name: 'Reports (group → cell, every tick)', color: '#3987e5' },
  { kind: 'senses', name: 'Answers to "what\'s near me?"', color: '#d95926' },
  { kind: 'views', name: 'Player views (cell → gateway)', color: '#199e70' },
  { kind: 'envelopes', name: 'Envelopes (inputs, actions, handoffs)', color: '#c98500' },
  { kind: 'debug', name: 'Debug (bus view data)', color: '#d55181' },
  { kind: 'control', name: 'Control (heartbeats, stats, commands)', color: '#898781' },
];

/** What this browser tab measured over the last second. */
export type ClientSample = {
  /** Main thread busy (our code + rendering), % of wall time. */
  cpuPct: number;
  /** JS heap in MB (Chrome only). */
  heapMb: number | null;
  down: Rate;
  up: Rate;
  fps: number;
};

const WINDOW_MS = 60_000;

// Validated categorical slots (dark mode). In the system charts colour follows the kind of
// process; in the per-group charts it follows the group (the same colours as its rings), with
// groups past the fifth folded into a grey "other groups" line.
const SLOTS = ['#3987e5', '#d95926', '#199e70', '#c98500', '#d55181'];
const OTHER = '#898781';
const COLOR = {
  groups: SLOTS[0],
  gateway: SLOTS[1],
  nats: SLOTS[2],
  spatial: SLOTS[3],
  client: SLOTS[4],
};
const INK = {
  primary: '#f0efec',
  secondary: '#c3c2b7',
  muted: '#898781',
  grid: '#2c2c2a',
  axis: '#383835',
  surface: '#16191e',
};

type Row = { t: number; v: Record<string, number | null> };
type Series = { key: string; name: string; color: string };
type ChartSpec = {
  title: string;
  series: Series[];
  format: (v: number) => string;
  /** Smallest top of the y axis, so near-zero noise doesn't fill the chart. */
  minMax: number;
  /** Extra line under the title (single-series charts show their current value here). */
  note?: (last: Row | undefined) => string;
  /** Current name per series key; a series with no name is hidden from the legend (for
   *  series that come and go, like one per group). */
  names?: () => Record<string, string | undefined>;
};

const kb = (bytes: number | null | undefined) => (bytes ?? 0) / 1024;

/** KB/s published per kind of bus message, summed over every process that reports it. */
function busKinds(procs: ProcessStats[]): Record<string, number | null> {
  const out: Record<string, number | null> = {};
  const reporting = procs.filter((p) => p.bus_kinds?.length);
  for (const k of BUS_KINDS) {
    out[`bus_${k.kind}`] = reporting.length
      ? kb(reporting.reduce((sum, p) => sum + (p.bus_kinds!.find(([kind]) => kind === k.kind)?.[1] ?? 0), 0))
      : null;
  }
  return out;
}
const fmt = (v: number, unit: string) => `${v < 10 ? v.toFixed(1) : Math.round(v).toLocaleString()}${unit}`;

/** Axis tick: a bare number (the unit is in the title), compact once the axis reaches 10,000. */
function axisLabel(v: number, top: number) {
  if (v === 0) return '0';
  if (top >= 10_000) return `${Number((v / 1000).toFixed(1))}K`;
  return v < 10 ? String(Number(v.toFixed(2))) : Math.round(v).toLocaleString();
}

/** Round up to 1, 2 or 5 times a power of ten. */
function niceMax(v: number) {
  const p = 10 ** Math.floor(Math.log10(v));
  return [1, 2, 5, 10].map((m) => m * p).find((m) => m >= v)!;
}

function div(cls: string, parent?: HTMLElement, text?: string) {
  const d = document.createElement('div');
  d.className = cls;
  if (text !== undefined) d.textContent = text;
  parent?.appendChild(d);
  return d;
}

class LineChart {
  private canvas = document.createElement('canvas');
  private noteEl: HTMLDivElement;
  private legendValues = new Map<string, HTMLSpanElement>();
  private legendItems = new Map<string, { item: HTMLDivElement; name: HTMLSpanElement }>();
  private hover: number | null = null;
  private static PLOT_H = 76;
  private static AXIS_H = 14;
  private static LEFT = 30;
  private static RIGHT = 8;

  constructor(private spec: ChartSpec, parent: HTMLElement, private rows: Row[], private tooltip: HTMLDivElement) {
    const card = div('chart', parent);
    div('chart-title', card, spec.title);
    this.noteEl = div('chart-note', card);
    card.appendChild(this.canvas);
    // A legend for two or more series (one series: the title already names it).
    if (spec.series.length > 1) {
      const legend = div('chart-legend', card);
      for (const s of spec.series) {
        const item = div('legend-item', legend);
        const key = div('line-key', item);
        key.style.background = s.color;
        const name = document.createElement('span');
        name.className = 'legend-name';
        name.textContent = s.name;
        const value = document.createElement('span');
        value.className = 'legend-value';
        item.append(name, value);
        this.legendValues.set(s.key, value);
        this.legendItems.set(s.key, { item, name });
      }
    }
    this.canvas.addEventListener('pointermove', (e) => this.onHover(e));
    this.canvas.addEventListener('pointerleave', () => {
      this.hover = null;
      this.tooltip.style.display = 'none';
      this.draw();
    });
  }

  private xOf(t: number, now: number, w: number) {
    const plotW = w - LineChart.LEFT - LineChart.RIGHT;
    return LineChart.LEFT + ((t - (now - WINDOW_MS)) / WINDOW_MS) * plotW;
  }

  draw() {
    const { canvas, spec, rows } = this;
    const w = canvas.parentElement!.clientWidth;
    const h = LineChart.PLOT_H + LineChart.AXIS_H;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(w * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      canvas.style.width = `${w}px`;
      canvas.style.height = `${h}px`;
    }
    const ctx = canvas.getContext('2d')!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);

    const last = rows[rows.length - 1];
    this.noteEl.textContent = spec.note?.(last) ?? '';
    if (spec.names) {
      const names = spec.names();
      for (const s of spec.series) {
        const entry = this.legendItems.get(s.key);
        if (!entry) continue;
        entry.item.style.display = names[s.key] ? '' : 'none';
        if (names[s.key] && entry.name.textContent !== names[s.key]) entry.name.textContent = names[s.key]!;
      }
    }
    for (const s of spec.series) {
      const v = last?.v[s.key];
      const legendValue = this.legendValues.get(s.key);
      if (legendValue) legendValue.textContent = v == null ? '-' : spec.format(v);
    }
    if (!last) return;

    const now = last.t;
    let top = spec.minMax;
    for (const r of rows) for (const s of spec.series) top = Math.max(top, r.v[s.key] ?? 0);
    top = niceMax(top);
    const plotTop = 6;
    const plotBottom = LineChart.PLOT_H;
    const y = (v: number) => plotBottom - (v / top) * (plotBottom - plotTop);

    // Recessive grid: hairlines at 0, half and top, labels in muted ink.
    ctx.font = '10px system-ui, sans-serif';
    ctx.textBaseline = 'middle';
    ctx.textAlign = 'right';
    for (const frac of [0, 0.5, 1]) {
      const gy = Math.round(y(top * frac)) + 0.5;
      ctx.strokeStyle = frac === 0 ? INK.axis : INK.grid;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(LineChart.LEFT, gy);
      ctx.lineTo(w - LineChart.RIGHT, gy);
      ctx.stroke();
      ctx.fillStyle = INK.muted;
      ctx.fillText(axisLabel(top * frac, top), LineChart.LEFT - 5, gy);
    }
    ctx.textBaseline = 'alphabetic';
    ctx.textAlign = 'left';
    ctx.fillText('60 s ago', LineChart.LEFT, h - 2);
    ctx.textAlign = 'right';
    ctx.fillText('now', w - LineChart.RIGHT, h - 2);

    // Series: 2px lines, broken where there is no value.
    ctx.lineWidth = 2;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    for (const s of spec.series) {
      ctx.strokeStyle = s.color;
      ctx.beginPath();
      let drawing = false;
      let end: { x: number; y: number } | null = null;
      for (const r of rows) {
        const v = r.v[s.key];
        if (v == null) {
          drawing = false;
          continue;
        }
        const px = this.xOf(r.t, now, w);
        const py = y(v);
        if (drawing) ctx.lineTo(px, py);
        else ctx.moveTo(px, py);
        drawing = true;
        end = { x: px, y: py };
      }
      ctx.stroke();
      // End dot with a ring in the surface colour.
      if (end && last.v[s.key] != null) {
        ctx.beginPath();
        ctx.arc(end.x, end.y, 4, 0, Math.PI * 2);
        ctx.fillStyle = s.color;
        ctx.fill();
        ctx.lineWidth = 2;
        ctx.strokeStyle = INK.surface;
        ctx.stroke();
      }
    }

    // Crosshair on the hovered sample.
    if (this.hover !== null && rows[this.hover]) {
      const hx = Math.round(this.xOf(rows[this.hover].t, now, w)) + 0.5;
      ctx.strokeStyle = INK.secondary;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(hx, plotTop);
      ctx.lineTo(hx, plotBottom);
      ctx.stroke();
    }
  }

  private onHover(e: PointerEvent) {
    const { rows, spec, tooltip } = this;
    if (!rows.length) return;
    const rect = this.canvas.getBoundingClientRect();
    const now = rows[rows.length - 1].t;
    // Snap to the nearest sample in time.
    let best = 0;
    for (let i = 0; i < rows.length; i++) {
      if (Math.abs(this.xOf(rows[i].t, now, rect.width) - (e.clientX - rect.left)) < Math.abs(this.xOf(rows[best].t, now, rect.width) - (e.clientX - rect.left))) best = i;
    }
    this.hover = best;
    this.draw();

    const row = rows[best];
    tooltip.replaceChildren();
    div('tip-time', tooltip, `${Math.round((now - row.t) / 1000)} s ago`);
    const names = spec.names?.();
    for (const s of spec.series) {
      if (names && !names[s.key]) continue;
      const line = div('tip-row', tooltip);
      const key = div('line-key', line);
      key.style.background = s.color;
      const value = document.createElement('b');
      value.textContent = row.v[s.key] == null ? '-' : spec.format(row.v[s.key]!);
      const name = document.createElement('span');
      name.textContent = names?.[s.key] ?? s.name;
      line.append(value, name);
    }
    tooltip.style.display = 'block';
    const left = Math.min(e.clientX + 14, window.innerWidth - tooltip.offsetWidth - 4);
    const above = e.clientY - tooltip.offsetHeight - 10;
    tooltip.style.left = `${left}px`;
    tooltip.style.top = `${above >= 4 ? above : e.clientY + 16}px`;
  }
}

/** Who the per-group and per-cell charts are about, from the debug overlay. */
export type PanelInfo = {
  /** Groups and Spatial Layer Cells in use, in order (slot i is drawn in colour i). */
  groups: () => number[];
  spatial: () => number[];
  name: (id: number) => string;
  /** CSS colour of a group's ring. */
  groupColor: (id: number) => string;
};

/** Bars for every group and Spatial Layer Cell: how much of each tick it spends working, now. */
class LoadBars {
  private el: HTMLDivElement;
  private header: HTMLDivElement;
  private columns: HTMLDivElement[];

  constructor(root: HTMLElement) {
    const box = div('load-now', root);
    this.header = div('chart-title', box);
    const cols = div('load-cols', box);
    this.columns = [div('load-col', cols), div('load-col', cols)];
    this.el = box;
  }

  draw(rows: { title: string; items: { name: string; color: string; load: number | null; detail: string; tip: string }[] }[]) {
    const all = rows.flatMap((r) => r.items.map((i) => i.load ?? 0));
    const top = Math.max(0, ...all);
    // A scale with room above the busiest: 10, 25, 50 or 100% of a tick.
    const scale = [10, 25, 50, 100].find((s) => top <= s * 0.9) ?? 100;
    this.header.textContent = `Load now: share of each tick spent working (bars 0 to ${scale}%)`;
    rows.forEach((r, c) => {
      const col = this.columns[c];
      const html =
        `<div class="load-title">${r.title}</div><div class="load-list">` +
        r.items
          .map((i) => {
            const pct = i.load == null ? '-' : `${i.load.toFixed(1)}%`;
            const w = i.load == null ? 0 : Math.min(100, (i.load / scale) * 100);
            return (
              `<div class="load-row" title="${i.tip}"><span class="load-name"><i style="background:${i.color}"></i>${i.name}</span>` +
              `<span class="load-track"><span class="load-bar" style="width:${w.toFixed(1)}%;background:${i.color}"></span></span>` +
              `<span class="load-val">${pct}</span><span class="load-detail">${i.detail}</span></div>`
            );
          })
          .join('') +
        '</div>';
      if (col.innerHTML !== html) col.innerHTML = html;
    });
    this.el.style.display = rows.some((r) => r.items.length) ? '' : 'none';
  }
}

/** The metrics panel: call `push` once a second with the latest server stats and the client's sample. */
export class MetricsPanel {
  private rows: Row[] = [];
  private charts: LineChart[] = [];
  private header: HTMLDivElement;
  private loadBars: LoadBars;
  private latest: ProcessStats[] = [];
  visible = false;

  constructor(
    private root: HTMLElement,
    private tickMs: () => number,
    private info: PanelInfo,
  ) {
    this.header = div('metrics-header', root);
    this.loadBars = new LoadBars(root);
    const grid = div('metrics-grid', root);
    const tooltip = div('metrics-tooltip', document.body);
    tooltip.style.display = 'none';

    const pct = (v: number) => fmt(v, '%');
    const mb = (v: number) => fmt(v, ' MB');
    const kbs = (v: number) => fmt(v, ' KB/s');
    const perSec = (v: number) => fmt(v, '/s');
    /** Names for the per-group slots: the first five groups in use, then "other groups". */
    const groupNames = (prefix: string, other: string) => () => {
      const names: Record<string, string | undefined> = {};
      const groups = this.info.groups();
      groups.slice(0, SLOTS.length).forEach((id, i) => (names[`${prefix}${i}`] = this.info.name(id)));
      if (groups.length > SLOTS.length) names[`${prefix}Other`] = `${other} (${groups.length - SLOTS.length} more)`;
      return names;
    };
    const groupSeries = (prefix: string) => [
      ...SLOTS.map((color, i) => ({ key: `${prefix}${i}`, name: `Group ${i + 1}`, color })),
      { key: `${prefix}Other`, name: 'Other groups', color: OTHER },
    ];
    const specs: ChartSpec[] = [
      {
        title: 'CPU (% of one core)',
        format: pct,
        minMax: 1,
        series: [
          { key: 'groupsCpu', name: 'Group processes (total)', color: COLOR.groups },
          { key: 'spatialCpu', name: 'Spatial Layer Cells (total)', color: COLOR.spatial },
          { key: 'gatewayCpu', name: 'Gateway', color: COLOR.gateway },
          { key: 'natsCpu', name: 'NATS', color: COLOR.nats },
          { key: 'clientCpu', name: 'Client main thread', color: COLOR.client },
        ],
        note: (r) => (r?.v.orchCpu != null ? `orchestrator ${pct(r.v.orchCpu)}` : ''),
      },
      {
        title: 'Memory (MB)',
        format: mb,
        minMax: 10,
        series: [
          { key: 'groupsMem', name: 'Group processes (total)', color: COLOR.groups },
          { key: 'spatialMem', name: 'Spatial Layer Cells (total)', color: COLOR.spatial },
          { key: 'gatewayMem', name: 'Gateway', color: COLOR.gateway },
          { key: 'natsMem', name: 'NATS', color: COLOR.nats },
          { key: 'clientMem', name: 'Client JS heap', color: COLOR.client },
        ],
        note: (r) => (r?.v.orchMem != null ? `orchestrator ${mb(r.v.orchMem)}` : ''),
      },
      {
        title: 'Server traffic (KB/s)',
        format: kbs,
        minMax: 1,
        series: [
          { key: 'groupsOut', name: 'Group processes → bus', color: COLOR.groups },
          { key: 'spatialOut', name: 'Spatial Layer Cells → bus', color: COLOR.spatial },
          { key: 'gatewayOut', name: 'Gateway → players', color: COLOR.gateway },
          { key: 'natsOut', name: 'NATS → subscribers', color: COLOR.nats },
        ],
      },
      {
        title: 'Bus traffic by kind (KB/s)',
        format: kbs,
        minMax: 1,
        series: BUS_KINDS.map((k) => ({ key: `bus_${k.kind}`, name: k.name, color: k.color })),
        note: (r) => {
          if (!r) return '';
          const total = BUS_KINDS.reduce((sum, k) => sum + (r.v[`bus_${k.kind}`] ?? 0), 0);
          return `all processes, ${kbs(total)} in total`;
        },
      },
      {
        title: 'Load per group (% of each tick)',
        format: pct,
        minMax: 1,
        series: groupSeries('g'),
        names: groupNames('g', 'Busiest of the others'),
      },
      {
        title: 'Load per Spatial Layer Cell (% of each tick)',
        format: pct,
        minMax: 1,
        series: SLOTS.map((color, i) => ({ key: `s${i}`, name: `Spatial ${String.fromCharCode(65 + i)}`, color })),
        names: () => {
          const names: Record<string, string | undefined> = {};
          this.info.spatial().slice(0, SLOTS.length).forEach((id, i) => (names[`s${i}`] = this.info.name(id)));
          return names;
        },
        note: () => 'answering questions and building views',
      },
      {
        title: 'Entities per group',
        format: (v: number) => fmt(v, ''),
        minMax: 5,
        series: groupSeries('e'),
        names: groupNames('e', 'Most of the others'),
      },
      {
        title: 'Spatial Layer Cells (per second)',
        format: perSec,
        minMax: 1,
        series: [
          { key: 'queries', name: 'Questions answered', color: SLOTS[2] },
          { key: 'views', name: 'Player views built', color: SLOTS[1] },
          { key: 'areas', name: 'Area actions passed on', color: SLOTS[0] },
        ],
      },
      {
        title: 'Actions across groups (per second)',
        format: perSec,
        minMax: 1,
        series: [{ key: 'hitsCross', name: 'Applied from another group', color: COLOR.groups }],
        note: (r) =>
          r && r.v.hitsCross != null
            ? `${perSec(r.v.hitsLocal ?? 0)} within one group · ${perSec(r.v.hitsForwarded ?? 0)} forwarded after a handoff`
            : 'no group reporting',
      },
      {
        title: 'Handoffs (per second)',
        format: perSec,
        minMax: 1,
        series: [{ key: 'handoffs', name: 'Entities handed off', color: COLOR.groups }],
        note: (r) => (r && r.v.handoffs != null ? 'only when groups are scaled or regrouped' : 'no group reporting'),
      },
      {
        title: 'Client download (KB/s)',
        format: kbs,
        minMax: 1,
        series: [{ key: 'clientDown', name: 'Download', color: COLOR.client }],
        note: (r) =>
          r ? `${kbs(r.v.clientDown ?? 0)} down, ${Math.round(r.v.clientDownMsgs ?? 0)} msg/s · up ${kbs(r.v.clientUp ?? 0)}` : '',
      },
    ];
    this.charts = specs.map((s) => new LineChart(s, grid, this.rows, tooltip));
  }

  setVisible(on: boolean) {
    this.visible = on;
    this.root.style.display = on ? 'block' : 'none';
    if (!on) (document.querySelector('.metrics-tooltip') as HTMLElement).style.display = 'none';
    if (on) this.draw();
  }

  /** Add one second of data. `server` is undefined when no stats arrived recently. */
  push(server: ProcessStats[] | undefined, client: ClientSample) {
    const procs = server ?? [];
    this.latest = procs;
    const hosts = procs.filter((p) => p.role === 'group-host');
    const groups = procs.filter((p) => p.role === 'group');
    const spatial = procs.filter((p) => p.role === 'spatial');
    const one = (role: string) => (role === 'gateway' ? gatewayTotals(procs) : procs.find((p) => p.role === role));
    const sum = (list: ProcessStats[], f: (p: ProcessStats) => number | null) =>
      list.length && list.some((p) => f(p) != null) ? list.reduce((a, p) => a + (f(p) ?? 0), 0) : null;

    const v: Row['v'] = {
      groupsCpu: sum(hosts, (p) => p.cpu_pct),
      spatialCpu: sum(spatial, (p) => p.cpu_pct),
      gatewayCpu: one('gateway')?.cpu_pct ?? null,
      natsCpu: one('nats')?.cpu_pct ?? null,
      orchCpu: one('orchestrator')?.cpu_pct ?? null,
      clientCpu: client.cpuPct,
      groupsMem: sum(hosts, (p) => p.mem_mb),
      spatialMem: sum(spatial, (p) => p.mem_mb),
      gatewayMem: one('gateway')?.mem_mb ?? null,
      natsMem: one('nats')?.mem_mb ?? null,
      orchMem: one('orchestrator')?.mem_mb ?? null,
      clientMem: client.heapMb,
      groupsOut: hosts.length ? kb(sum(hosts, (p) => p.bus_out.bytes)) : null,
      spatialOut: spatial.length ? kb(sum(spatial, (p) => p.bus_out.bytes)) : null,
      ...busKinds(procs),
      gatewayOut: one('gateway') ? kb(one('gateway')!.ws_out.bytes) : null,
      natsOut: one('nats') ? kb(one('nats')!.bus_out.bytes) : null,
      clientDown: kb(client.down.bytes),
      clientDownMsgs: client.down.msgs,
      clientUp: kb(client.up.bytes),
      clientUpMsgs: client.up.msgs,
      queries: sum(spatial, (p) => p.queries),
      views: sum(spatial, (p) => p.views),
      areas: sum(spatial, (p) => p.areas),
      hitsCross: sum(groups, (p) => p.actions_received),
      hitsLocal: sum(groups, (p) => p.actions_local),
      hitsForwarded: sum(groups, (p) => p.actions_forwarded),
      handoffs: sum(groups, (p) => p.handoffs),
    };
    // Per group: the first five in use get their own line; the rest fold into "other groups"
    // (the busiest of them for load, the most entities for entities).
    const inUse = this.info.groups();
    const stat = (id: number) => groups.find((g) => g.id === id);
    inUse.slice(0, SLOTS.length).forEach((id, i) => {
      v[`g${i}`] = stat(id)?.load_pct ?? null;
      v[`e${i}`] = stat(id)?.players ?? null;
    });
    const rest = inUse.slice(SLOTS.length).map(stat).filter((s): s is ProcessStats => !!s);
    v.gOther = rest.length ? Math.max(...rest.map((s) => s.load_pct ?? 0)) : null;
    v.eOther = rest.length ? Math.max(...rest.map((s) => s.players)) : null;
    this.info.spatial().slice(0, SLOTS.length).forEach((id, i) => {
      v[`s${i}`] = spatial.find((s) => s.id === id)?.load_pct ?? null;
    });
    const t = performance.now();
    this.rows.push({ t, v });
    while (this.rows.length && this.rows[0].t < t - WINDOW_MS) this.rows.shift();

    const gateways = gatewayTotals(procs);
    const players = gateways?.players;
    this.header.textContent =
      `${Math.round(1000 / this.tickMs())} ticks/s (${this.tickMs()} ms)` +
      ` · ${players ?? '?'} player${players === 1 ? '' : 's'}` +
      (gateways && gateways.count > 1 ? ` on ${gateways.count} gateways` : '') +
      ` · ${groups.length} Entity Group${groups.length === 1 ? '' : 's'} on ${hosts.length} process${hosts.length === 1 ? '' : 'es'}, ${spatial.length} Spatial Layer Cell${spatial.length === 1 ? '' : 's'}` +
      ` · client ${Math.round(client.fps)} fps`;
    if (this.visible) this.draw();
  }

  private draw() {
    const groups = this.latest.filter((p) => p.role === 'group');
    const spatial = this.latest.filter((p) => p.role === 'spatial');
    const ms = (v: number | null) => (v == null ? '-' : `${v.toFixed(2)} ms`);
    const short = (id: number) => this.info.name(id).replace(/ \(.*\)/, '');
    const tip = (s: ProcessStats | undefined, id: number, extra: string) =>
      s ? `${this.info.name(id)}: ${(s.load_pct ?? 0).toFixed(1)}% of each tick (average ${ms(s.tick_ms_avg)}, worst ${ms(s.tick_ms_max)}). ${extra}` : this.info.name(id);
    this.loadBars.draw([
      {
        title: 'Server Entity Groups',
        items: this.info.groups().map((id) => {
          const s = groups.find((g) => g.id === id);
          return {
            name: short(id),
            color: this.info.groupColor(id),
            load: s?.load_pct ?? null,
            detail: s ? `${s.players} entities` : '',
            tip: tip(s, id, s ? `${s.players} entities, ${s.handoffs.toFixed(1)} handoffs/s, ${s.queries.toFixed(1)} questions/s` : ''),
          };
        }),
      },
      {
        title: 'Spatial Layer Cells',
        items: this.info.spatial().map((id) => {
          const s = spatial.find((c) => c.id === id);
          return {
            name: short(id),
            color: '#c3c2b7',
            load: s?.load_pct ?? null,
            detail: s ? `${s.players} held · ${s.viewers} watching` : '',
            tip: tip(s, id, s ? `${s.players} held, ${s.viewers} watching, ${s.queries.toFixed(0)} questions/s, ${s.views.toFixed(0)} views/s` : ''),
          };
        }),
      },
    ]);
    for (const c of this.charts) c.draw();
  }
}
