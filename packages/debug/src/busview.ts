// Debug view of the bus: every server process is a node around NATS, and messages travel
// sender -> NATS -> receiver(s), the way they really do. Slowed down so you can follow them
// (a real hop takes about a millisecond).
//
// Server Entity Groups are circles on the left, in their group colour; Spatial Layer Cells are
// squares on the right. Exact, per envelope: handoffs, players' inputs and commands, actions,
// area actions (group -> Spatial Layer Cell -> groups with someone in the circle), forwards.
// Every-tick streams, with their real senders, receivers and sizes but sampled: groups' reports
// to Spatial Layer Cells, spatial query results, players' views, and where players look.
//
// Under the legend, the last few actions are traced hop by hop (every envelope one action
// leads to shares a trace id). The game says which of its events start and end a trace.

import { isSpatial, type BusMsg, type ProcessStats, type StreamMsg } from '@worldmesh/client';
import { gatewayTotals } from './gateways.ts';

/** What a game event says about a trace, for the "recent actions" list. */
export type TraceNote = {
  trace: number;
  /** The event that started it: who did what (e.g. "Player 3 bow → Bot 12"), from which group. */
  start?: { label: string; group: number; area?: boolean };
  /** A result: a short text (e.g. "hit 12", "out of range") and whether it landed. */
  result?: { text: string; landed: boolean };
};

type Kind = 'handoff' | 'action' | 'senses' | 'forward' | 'player' | 'report' | 'view' | 'control';

// Validated categorical slots (dark mode) for envelopes; greys for the background streams.
const KINDS: { kind: Kind; name: string; color: string }[] = [
  { kind: 'handoff', name: 'Handoff', color: '#3987e5' },
  { kind: 'action', name: 'Action (entity, area or everyone)', color: '#d95926' },
  { kind: 'senses', name: 'Spatial query results', color: '#199e70' },
  { kind: 'forward', name: 'Forwarded', color: '#c98500' },
  { kind: 'player', name: 'Player input or command', color: '#d55181' },
  { kind: 'report', name: 'Group report', color: '#898781' },
  { kind: 'view', name: 'Views, viewers', color: '#6b6a65' },
  { kind: 'control', name: 'Heartbeat, assignment', color: '#4a4945' },
];
const COLOR = Object.fromEntries(KINDS.map((k) => [k.kind, k.color])) as Record<Kind, string>;
const BIG: ReadonlySet<Kind> = new Set(['handoff', 'action', 'senses', 'forward', 'player']);
const INK = { primary: '#f0efec', secondary: '#c3c2b7', muted: '#898781', spoke: '#2c2c2a', surface: '#16191e', node: '#23272e' };

/** One leg takes this long on screen; a message is two legs (to NATS, then out). */
const LEG_MS = 320;
/** Each stream (sender, receiver, kind) is shown at most this often. */
const STREAM_EVERY_MS = 350;
const GATEWAY = -1;
const ORCHESTRATOR = -2;
const NATS = -3;

type Dot = { kind: Kind; from: number; to: number[]; born: number };
type Hop = { from: number; to: number; kind: BusMsg['kind'] };

export class BusView {
  private canvas = document.createElement('canvas');
  private counts = new Map<Kind, HTMLSpanElement>();
  private tooltip: HTMLDivElement;
  private dots: Dot[] = [];
  private nodes = new Map<number, { x: number; y: number }>();
  private nodeKey = '';
  private groups: number[] = [];
  private spatial: number[] = [];
  private groupsInUse: number[] = [];
  private spatialInUse: number[] = [];
  private stats = new Map<number, ProcessStats>();
  /** Every gateway's numbers, added up: the bus view draws them as one. */
  private gatewayStats: (ProcessStats & { count: number }) | undefined;
  private orchestratorStats: ProcessStats | undefined;
  private natsStats: ProcessStats | undefined;
  /** Recent message times per kind, for the per-second counts in the legend. */
  private recent = new Map<Kind, number[]>();
  private lastStream = new Map<string, number>();
  private hover: number | null = null;
  private tracesEl: HTMLDivElement;
  /** Per trace: when first seen, the hops it took, how it started and ended. */
  private traces = new Map<number, { at: number; hops: Hop[]; start?: TraceNote['start']; results: NonNullable<TraceNote['result']>[] }>();
  visible = false;

  constructor(
    private root: HTMLElement,
    private groupColor: (id: number) => number,
    private nameOf: (id: number) => string,
  ) {
    const header = document.createElement('div');
    header.className = 'bus-header';
    header.textContent = 'Bus traffic';
    const sub = document.createElement('div');
    sub.className = 'bus-sub';
    sub.textContent = 'Groups ● left, Spatial Layer Cells ■ right. Everything goes through NATS. Slowed down: a real hop takes ~1 ms; tick streams sampled.';
    root.append(header, sub, this.canvas);
    const legend = document.createElement('div');
    legend.className = 'bus-legend';
    for (const k of KINDS) {
      const item = document.createElement('div');
      item.className = 'legend-item';
      const key = document.createElement('span');
      key.className = 'dot-key';
      key.style.background = k.color;
      const name = document.createElement('span');
      name.className = 'legend-name';
      name.textContent = k.name;
      const count = document.createElement('span');
      count.className = 'legend-value';
      item.append(key, name, count);
      legend.appendChild(item);
      this.counts.set(k.kind, count);
    }
    root.appendChild(legend);
    const tracesTitle = document.createElement('div');
    tracesTitle.className = 'bus-sub';
    tracesTitle.style.marginTop = '6px';
    tracesTitle.textContent = 'Recent actions, hop by hop';
    this.tracesEl = document.createElement('div');
    this.tracesEl.className = 'bus-traces';
    root.append(tracesTitle, this.tracesEl);
    this.tooltip = document.createElement('div');
    this.tooltip.className = 'metrics-tooltip';
    this.tooltip.style.display = 'none';
    document.body.appendChild(this.tooltip);
    this.canvas.addEventListener('pointermove', (e) => this.onHover(e));
    this.canvas.addEventListener('pointerleave', () => {
      this.hover = null;
      this.tooltip.style.display = 'none';
    });
    const frame = () => {
      if (this.visible) this.draw();
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  }

  setVisible(on: boolean) {
    this.visible = on;
    this.root.style.display = on ? 'block' : 'none';
    if (!on) this.tooltip.style.display = 'none';
  }

  /** Processes in use, in order; idle ones (known only from their stats) go after them. */
  setProcesses(groups: number[], spatial: number[]) {
    this.groupsInUse = groups;
    this.spatialInUse = spatial;
    const idle = (role: string, inUse: number[]) =>
      [...this.stats.values()].filter((p) => p.role === role && !inUse.includes(p.id)).map((p) => p.id).sort((a, b) => a - b);
    this.groups = [...groups, ...idle('group', groups)];
    this.spatial = [...spatial, ...idle('spatial', spatial)];
  }

  onStats(processes: ProcessStats[]) {
    this.stats.clear();
    this.gatewayStats = gatewayTotals(processes);
    for (const p of processes) {
      if (p.role === 'group' || p.role === 'spatial') this.stats.set(p.id, p);
      if (p.role === 'orchestrator') this.orchestratorStats = p;
      if (p.role === 'nats') this.natsStats = p;
    }
    this.setProcesses(this.groupsInUse, this.spatialInUse);
    // Heartbeats: every group and Spatial Layer Cell tells the orchestrator twice a second.
    const now = performance.now();
    for (const id of this.stats.keys()) {
      this.spawn('control', id, [ORCHESTRATOR], now + Math.random() * 500);
      this.spawn('control', id, [ORCHESTRATOR], now + 500 + Math.random() * 500);
    }
  }

  /** A new assignment: the orchestrator publishes it to everyone. */
  onOwnership() {
    if (this.visible) this.spawn('control', ORCHESTRATOR, [...this.groups, ...this.spatial, GATEWAY], performance.now());
  }

  /** One tick's snapshot from the gateway. */
  onSnapshot(bus: BusMsg[], streams: StreamMsg[], notes: TraceNote[]) {
    const now = performance.now();
    this.recordTraces(bus, notes, now);
    if (!this.visible) return;
    for (const m of bus) {
      const kind: Kind =
        m.kind === 'area' || m.kind === 'action' || m.kind === 'broadcast' ? 'action' : m.kind === 'command' || m.kind === 'input' ? 'player' : m.kind;
      this.spawn(kind, m.from === 0 ? GATEWAY : m.from, [m.to], now);
    }
    for (const s of streams) {
      if (s.kind !== 'report' && s.n === 0) continue; // reports go out even empty; the rest only with content
      const key = `${s.from}>${s.to}:${s.kind}`;
      if (now - (this.lastStream.get(key) ?? 0) < STREAM_EVERY_MS) continue;
      this.lastStream.set(key, now);
      const kind: Kind = s.kind === 'report' ? 'report' : s.kind === 'senses' ? 'senses' : 'view';
      this.spawn(kind, s.from === 0 ? GATEWAY : s.from, [s.to === 0 ? GATEWAY : s.to], now);
    }
  }

  /** Group envelopes and events by trace and list the latest actions that crossed the bus. */
  private recordTraces(bus: BusMsg[], notes: TraceNote[], now: number) {
    const entry = (trace: number) => {
      let t = this.traces.get(trace);
      if (!t) this.traces.set(trace, (t = { at: now, hops: [], results: [] }));
      return t;
    };
    for (const m of bus) if (m.trace && m.kind !== 'input') entry(m.trace).hops.push({ from: m.from === 0 ? GATEWAY : m.from, to: m.to, kind: m.kind });
    for (const n of notes) {
      if (!n.trace) continue;
      const t = entry(n.trace);
      if (n.start) t.start = n.start;
      if (n.result) t.results.push(n.result);
    }
    for (const [id, t] of this.traces) if (now - t.at > 8000) this.traces.delete(id);
    if (!this.visible) return;

    const name = (id: number) => this.label(id);
    const uniq = (ids: number[]) => [...new Set(ids)];
    const lines = [...this.traces.values()]
      .filter((t) => t.start && t.hops.length > 0)
      .sort((a, b) => b.at - a.at)
      .slice(0, 5)
      .map((t) => {
        const start = t.start!;
        const requested = t.hops.some((h) => h.kind === 'command');
        // (A group still listening for an entity it just handed off hears its own action; skip that.)
        const direct = uniq(t.hops.filter((h) => (h.kind === 'action' || h.kind === 'broadcast') && h.from !== h.to).map((h) => h.to));
        const cells = uniq(t.hops.filter((h) => h.kind === 'area' && isSpatial(h.to)).map((h) => h.to));
        const reached = uniq(t.hops.filter((h) => h.kind === 'area' && isSpatial(h.from)).map((h) => h.to));
        const forwards = t.hops.filter((h) => h.kind === 'forward').map((h) => name(h.to));
        let route = (requested ? 'Gateway → ' : '') + name(start.group);
        if (direct.length) route += ` → ${direct.map(name).join(', ')}`;
        if (cells.length) route += ` → ${cells.map(name).join(', ')} → ${reached.length ? reached.map(name).join(', ') : 'nobody there'}`;
        if (forwards.length) route += `, forwarded to ${forwards.join(', ')}`;
        const results = t.results;
        const outcome = !results.length
          ? (cells.length && !reached.length ? 'no one there' : '…')
          : results.length > 1 || start.area
            ? `${results.filter((r) => r.landed).length} landed`
            : results[0].text;
        return `${start.label}: ${route} · ${outcome}`;
      });
    const text = lines.join('\n');
    if (this.tracesEl.textContent !== text) this.tracesEl.textContent = text;
  }

  private spawn(kind: Kind, from: number, to: number[], born: number) {
    if (!to.length) return;
    this.dots.push({ kind, from, to, born });
    const list = this.recent.get(kind) ?? [];
    list.push(born);
    this.recent.set(kind, list);
  }

  /** NATS in the middle, gateway on top, orchestrator at the bottom, groups on the left arc,
   *  Spatial Layer Cells on the right arc. */
  private layout(w: number, h: number) {
    const key = `${w}x${h}:${this.groups.join(',')}|${this.spatial.join(',')}`;
    if (key === this.nodeKey) return;
    this.nodeKey = key;
    this.nodes.clear();
    const cx = w / 2;
    const cy = h / 2;
    const rx = w / 2 - 40;
    const ry = h / 2 - 30;
    this.nodes.set(NATS, { x: cx, y: cy });
    this.nodes.set(GATEWAY, { x: cx, y: cy - ry });
    this.nodes.set(ORCHESTRATOR, { x: cx, y: cy + ry });
    const arc = (ids: number[], side: number) =>
      ids.forEach((id, i) => {
        const t = (i + 1) / (ids.length + 1); // 0..1 from top to bottom
        const angle = (t - 0.5) * Math.PI * 0.78; // stays clear of the gateway and orchestrator
        this.nodes.set(id, { x: cx + side * Math.cos(angle) * rx, y: cy + Math.sin(angle) * ry });
      });
    arc(this.groups, -1);
    arc(this.spatial, 1);
  }

  private label(id: number) {
    if (id === GATEWAY) return (this.gatewayStats?.count ?? 1) > 1 ? `Gateways (${this.gatewayStats!.count})` : 'Gateway';
    if (id === ORCHESTRATOR) return 'Orchestrator';
    if (id === NATS) return 'NATS';
    return this.nameOf(id).replace(/ \(.*\)/, '');
  }

  private draw() {
    const { canvas } = this;
    const w = this.root.clientWidth - 24;
    const h = 320;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      canvas.style.width = `${w}px`;
      canvas.style.height = `${h}px`;
    }
    this.layout(w, h);
    const ctx = canvas.getContext('2d')!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const now = performance.now();
    const hub = this.nodes.get(NATS)!;

    // Spokes: every process connects only to NATS.
    ctx.strokeStyle = INK.spoke;
    ctx.lineWidth = 1;
    for (const [id, p] of this.nodes) {
      if (id === NATS) continue;
      ctx.beginPath();
      ctx.moveTo(hub.x, hub.y);
      ctx.lineTo(p.x, p.y);
      ctx.stroke();
    }

    // Messages: first leg sender -> NATS, second leg NATS -> each receiver.
    this.dots = this.dots.filter((d) => now - d.born < LEG_MS * 2);
    for (const d of this.dots) {
      const age = now - d.born;
      if (age < 0) continue;
      const from = this.nodes.get(d.from);
      if (!from) continue;
      const paths = age < LEG_MS ? [[from, hub]] : d.to.map((t) => [hub, this.nodes.get(t)]).filter((p) => p[1]);
      const k = (age % LEG_MS) / LEG_MS;
      const eased = k * k * (3 - 2 * k);
      for (const [a, b] of paths as { x: number; y: number }[][]) {
        const x = a.x + (b.x - a.x) * eased;
        const y = a.y + (b.y - a.y) * eased;
        ctx.beginPath();
        ctx.arc(x, y, BIG.has(d.kind) ? 5 : 3.5, 0, Math.PI * 2);
        ctx.fillStyle = COLOR[d.kind];
        ctx.fill();
        ctx.lineWidth = 2;
        ctx.strokeStyle = INK.surface;
        ctx.stroke();
      }
    }

    // Nodes on top: circles for groups and the other processes, squares for Spatial Layer Cells.
    // Many groups: smaller circles, numbers only when they fit, no captions.
    ctx.textAlign = 'center';
    const groupR = Math.max(6, Math.min(20, (h - 60) / Math.max(1, this.groups.length) / 1.25));
    for (const [id, p] of this.nodes) {
      const spatial = isSpatial(id);
      const group = id > 0 && !spatial;
      const inUse = group ? this.groupsInUse.includes(id) : spatial ? this.spatialInUse.includes(id) : true;
      const r = id === NATS ? 26 : group ? groupR : 20;
      ctx.beginPath();
      if (spatial) ctx.roundRect(p.x - r, p.y - r, r * 2, r * 2, 5);
      else ctx.arc(p.x, p.y, r, 0, Math.PI * 2);
      ctx.fillStyle = INK.node;
      ctx.fill();
      ctx.lineWidth = group ? 3 : spatial ? 2 : 1.5;
      ctx.strokeStyle = group
        ? `#${this.groupColor(id).toString(16).padStart(6, '0')}`
        : spatial
          ? INK.secondary
          : this.hover === id
            ? INK.primary
            : INK.muted;
      if (!inUse) ctx.setLineDash([3, 3]);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = inUse ? INK.primary : INK.muted;
      ctx.textBaseline = 'middle';
      // Inside: just the number or letter for groups and cells (the side says which).
      const name = this.label(id);
      const inside = group || spatial ? name.replace(/^(Group|Spatial) /, '') : name;
      ctx.font = group || spatial ? `700 ${r >= 14 ? 13 : 9}px system-ui, sans-serif` : '600 11px system-ui, sans-serif';
      if (!group || r >= 9) ctx.fillText(inside, p.x, p.y);
      const s = this.stats.get(id);
      if (s && (!group || this.groups.length <= 8)) {
        ctx.fillStyle = INK.muted;
        ctx.font = '10px system-ui, sans-serif';
        const below = !inUse ? 'idle' : spatial ? `${s.players} held` : `${s.players} entities`;
        ctx.fillText(below, p.x, p.y + r + 10);
      }
    }

    // Per-second counts in the legend (last 5 s).
    for (const k of KINDS) {
      const list = (this.recent.get(k.kind) ?? []).filter((t) => now - t < 5000);
      this.recent.set(k.kind, list);
      const span = this.counts.get(k.kind)!;
      const text = `${(list.length / 5).toFixed(1)}/s`;
      if (span.textContent !== text) span.textContent = text;
    }
  }

  private onHover(e: PointerEvent) {
    const rect = this.canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    let best: number | null = null;
    for (const [id, p] of this.nodes) {
      if (Math.hypot(p.x - mx, p.y - my) < (best === null ? 26 : 12)) best = id;
    }
    this.hover = best;
    if (best === null) {
      this.tooltip.style.display = 'none';
      return;
    }
    const s =
      best === GATEWAY ? this.gatewayStats : best === ORCHESTRATOR ? this.orchestratorStats : best === NATS ? this.natsStats : this.stats.get(best);
    const lines: string[] = [best > 0 ? this.nameOf(best) : this.label(best)];
    if (s) {
      if (s.role !== 'group') lines.push(`bus in ${Math.round(s.bus_in.msgs)} msgs/s · out ${Math.round(s.bus_out.msgs)} msgs/s`);
      if (s.load_pct != null) lines.push(`load ${s.load_pct.toFixed(1)}% of each tick`);
      if (s.role === 'group') {
        lines.push(`${s.players} entities simulated · ${s.queries.toFixed(1)} questions/s asked`);
        lines.push(`actions: ${s.actions_local.toFixed(1)}/s within the group, ${s.actions_received.toFixed(1)}/s from other groups, ${s.actions_forwarded.toFixed(1)}/s forwarded`);
        lines.push(`${s.handoffs.toFixed(1)} handoffs/s`);
      } else if (s.role === 'spatial') {
        lines.push(`${s.players} entities held (copies) · ${s.viewers} players watching`);
        lines.push(`${s.queries.toFixed(1)} questions/s answered · ${s.views.toFixed(1)} views/s · ${s.areas.toFixed(1)} area actions/s`);
      }
    } else if (best === NATS) {
      lines.push('no monitoring data');
    }
    this.tooltip.replaceChildren(
      ...lines.map((text, i) => {
        const div = document.createElement('div');
        if (i === 0) div.className = 'tip-time';
        div.textContent = text;
        return div;
      }),
    );
    this.tooltip.style.display = 'block';
    this.tooltip.style.left = `${Math.min(e.clientX + 14, window.innerWidth - this.tooltip.offsetWidth - 4)}px`;
    this.tooltip.style.top = `${Math.max(4, e.clientY - this.tooltip.offsetHeight - 10)}px`;
  }
}
