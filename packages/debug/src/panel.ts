// The debug panel: see and change how the world is split while you play.
//
// It builds its own DOM (a panel on the right, plus the metrics and bus views) and follows a
// WorldmeshClient. Scaling and fake lag need the gateway to run with `--allow-debug`.
//
// Drawing the world is the game's job, so the panel doesn't draw overlays itself: it keeps
// their on/off state in `overlays` (groups, spatial areas, questions, view, raw positions) and
// tells you when one changes, and gives you names and colours for every group and cell so
// your overlays match its legends. Game controls go in sections you add with `addSection`.

import type { Assignment, GameEvent, ProcessStats, Snapshot, WorldmeshClient } from '@worldmesh/client';
import { isSpatial } from '@worldmesh/client';

import { BusView, type TraceNote } from './busview.ts';
import { MetricsPanel, type ClientSample } from './metrics.ts';
import { injectStyles } from './styles.ts';

/** Categorical group colours (validated dark palette), by the group's place in the assignment. */
export const GROUP_COLORS = [0x3987e5, 0xd95926, 0x199e70, 0xc98500, 0xd55181, 0x9b5de5, 0x00bbf9, 0xcaffbf];
export const NO_GROUP_COLOR = 0x8a8a8a;

export type Overlay = 'groups' | 'spatial' | 'senses' | 'view' | 'ghosts';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyClient = WorldmeshClient<any, any, any, any, any, any>;

export type DebugPanelOptions = {
  client: AnyClient;
  /** Where to put the panels. Default: document.body. */
  parent?: HTMLElement;
  /** Keyboard shortcuts (T, I, O, L, N, V, P, G, M, B). Default true. */
  keys?: boolean;
  /** Called when an overlay is switched on or off. */
  onOverlay?(overlay: Overlay, on: boolean): void;
  /** What one of the game's events (`{ name, data }`) says about its trace, for the bus
   *  view's list of recent actions (who did what, and how it ended). */
  traceNote?(event: GameEvent): TraceNote | undefined;
};

const css = (color: number) => `#${color.toString(16).padStart(6, '0')}`;
const hex4 = (id: number) => id.toString(16).padStart(4, '0');

function make<K extends keyof HTMLElementTagNameMap>(tag: K, parent: HTMLElement, cls = '', html = ''): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (cls) el.className = cls;
  if (html) el.innerHTML = html;
  parent.appendChild(el);
  return el;
}

export class DebugPanel {
  readonly root: HTMLDivElement;
  readonly metrics: MetricsPanel;
  readonly bus: BusView;
  /** A canvas for the game's minimap, shown while the "view" overlay is on. */
  readonly minimap: HTMLCanvasElement;
  readonly overlays: Record<Overlay, boolean> = { groups: true, spatial: true, senses: false, view: false, ghosts: false };
  assignment: Assignment | undefined;
  /** Latest stats per group or Spatial Layer Cell. */
  readonly processStats = new Map<number, ProcessStats>();

  private client: AnyClient;
  private options: DebugPanelOptions;
  private boxes = new Map<string, HTMLInputElement>();
  private latestStats: { at: number; processes: ProcessStats[] } | undefined;
  private frames = { count: 0, busyMs: 0 };
  private els: Record<string, HTMLElement> = {};

  constructor(options: DebugPanelOptions) {
    injectStyles();
    this.options = options;
    this.client = options.client;
    // Every process's numbers, once a second: the gateway only sends them to players who ask.
    this.client.setSettings({ stats: true });
    const parent = options.parent ?? document.body;
    this.root = make('div', parent, 'wm-debug');
    this.metrics = new MetricsPanel(make('div', parent, 'wm-metrics'), () => this.client.tickMs, {
      groups: () => this.assignment?.groups ?? [],
      spatial: () => this.assignment?.spatial ?? [],
      name: (id) => this.processName(id),
      groupColor: (id) => css(this.groupColor(id)),
    });
    this.bus = new BusView(make('div', parent, 'wm-bus'), (id) => this.groupColor(id), (id) => this.processName(id));
    this.minimap = this.build();

    this.client.listen({
      ownership: (a) => this.applyAssignment(a),
      stats: (processes) => this.applyStats(processes),
      snapshot: (s) => this.applySnapshot(s),
    });
    if (options.keys !== false) window.addEventListener('keydown', (e) => this.onKey(e));
    setInterval(() => this.everySecond(), 1000);
    setInterval(() => this.updatePrediction(), 250);
    const frame = () => {
      this.frames.count++;
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  }

  // ---- Names and colours, matching the legends ----

  groupIndex(id: number) {
    return this.assignment ? this.assignment.groups.indexOf(id) : -1;
  }
  spatialIndex(id: number) {
    return this.assignment ? this.assignment.spatial.indexOf(id) : -1;
  }
  groupColor(id: number) {
    const i = this.groupIndex(id);
    return i < 0 ? NO_GROUP_COLOR : GROUP_COLORS[i % GROUP_COLORS.length];
  }
  groupName(id: number) {
    const i = this.groupIndex(id);
    return i < 0 ? `Group ${hex4(id)}` : `Group ${i + 1} (${hex4(id)})`;
  }
  spatialLetter(i: number) {
    return String.fromCharCode(65 + (i % 26));
  }
  spatialName(id: number) {
    const i = this.spatialIndex(id);
    return i < 0 ? `Spatial ${hex4(id)}` : `Spatial ${this.spatialLetter(i)} (${hex4(id)})`;
  }
  /** Any group's or Spatial Layer Cell's name, by id. */
  processName(id: number) {
    return isSpatial(id) ? this.spatialName(id) : this.groupName(id);
  }
  /** The name without the id ("Group 2", "Spatial B"). */
  short(id: number) {
    return this.processName(id).replace(/ \(.*\)/, '');
  }
  lagOf(group: number) {
    return this.assignment?.group_lag?.find(([g]) => g === group)?.[1] ?? 0;
  }

  /** A section of your own at the end of the panel (game controls, stats). */
  addSection(title: string): HTMLDivElement {
    make('div', this.root, 'title', title).style.marginTop = '10px';
    return make('div', this.root);
  }

  /** Time your frame took on the main thread (ms), for the metrics' client CPU. Optional. */
  recordFrame(busyMs: number) {
    this.frames.busyMs += busyMs;
  }

  /** Switch an overlay or view on or off (as its checkbox or key does). */
  set(name: Overlay | 'compact' | 'binaryBus' | 'predict' | 'metrics' | 'bus', on: boolean) {
    const box = this.boxes.get(name);
    if (box) box.checked = on;
    switch (name) {
      case 'compact':
        this.client.setSettings({ compact: on });
        return;
      case 'binaryBus':
        this.client.sendDebug({ binary_bus: on });
        return;
      case 'predict':
        if (this.client.predictor) {
          this.client.predictor.enabled = on;
          this.client.predictor.reset();
        }
        return;
      case 'metrics':
        this.metrics.setVisible(on);
        return;
      case 'bus':
        this.bus.setVisible(on);
        this.sendDebugSetting();
        return;
    }
    this.overlays[name] = on;
    if (name === 'senses') this.sendDebugSetting();
    if (name === 'view') this.minimap.style.display = on ? 'block' : 'none';
    this.options.onOverlay?.(name, on);
  }

  private sendDebugSetting() {
    this.client.setSettings({ debug: this.bus.visible || this.overlays.senses });
  }

  private build(): HTMLCanvasElement {
    const r = this.root;
    const checkbox = (name: Parameters<DebugPanel['set']>[0], label: string, key: string, on: boolean, title = '') => {
      const l = make('label', r, '', `<input type="checkbox" ${on ? 'checked' : ''}/> ${label} <kbd>${key}</kbd>`);
      if (title) l.title = title;
      const box = l.querySelector('input')!;
      this.boxes.set(name, box);
      box.addEventListener('change', () => {
        this.set(name, box.checked);
        box.blur();
      });
      make('br', r);
    };
    const button = (parent: HTMLElement, text: string, title: string, onClick: () => void, wide = false) => {
      const b = make('button', parent, wide ? 'wide' : '', text);
      b.title = title;
      b.addEventListener('click', () => {
        onClick();
        b.blur(); // so keys go back to the game
      });
      return b;
    };
    const muted = (cls: string) => {
      const d = make('div', r, `muted ${cls}`);
      d.style.fontSize = '11px';
      this.els[cls] = d;
      return d;
    };
    const debug = (cmd: Parameters<AnyClient['sendDebug']>[0]) => this.client.sendDebug(cmd);

    make('div', r, 'title', 'Server Entity Groups');
    checkbox('groups', 'Colour entities by group', 'O', true);
    const groupsRow = make('div', r, 'row', 'Groups ');
    button(groupsRow, '-', 'Use one fewer group (its entities are handed off)', () =>
      this.assignment && debug({ set_groups: Math.max(1, this.assignment.groups.length - 1) }),
    );
    const count = make('input', groupsRow, 'wm-groups-count');
    Object.assign(count, { type: 'number', min: '1', max: '1000', value: '2', title: 'Type how many groups to use, then Enter' });
    count.addEventListener('change', () => {
      const n = Math.round(Number(count.value));
      if (n >= 1) debug({ set_groups: Math.min(1000, n) });
      count.blur();
    });
    this.els.groupsCount = count;
    button(groupsRow, '+', 'Use one more group (some entities are handed to it)', () =>
      this.assignment && debug({ set_groups: this.assignment.groups.length + 1 }),
    );
    muted('groups-running');
    button(r, 'Regroup by position', 'Re-split entities between groups by where they stand now', () => debug({ regroup: true }), true);
    const legend = make('div', r, 'legend wm-group-legend');
    this.els.groupLegend = legend;
    const lagBox = make('div', r);
    lagBox.title =
      'Hold back everything this group sends and receives by this long (one way), to see how a slow link to one group plays out. Click a group above to pick it. 0 turns it off';
    make('div', lagBox, 'muted', 'Fake lag for one group (one way)').style.fontSize = '11px';
    const lagRow = make('div', lagBox, 'row');
    const picker = make('select', lagRow, 'wm-lag-group');
    const ms = make('input', lagRow, 'wm-lag-ms');
    Object.assign(ms, { type: 'number', min: '0', max: '5000', step: '50', value: '200' });
    lagRow.append(' ms ');
    const setLag = () => {
      const group = Number(picker.value);
      const lag = Math.max(0, Math.min(5000, Math.round(Number(ms.value) || 0)));
      if (group) debug({ set_lag: [group, lag] });
    };
    button(lagRow, 'Set', 'Apply the fake lag to the picked group', setLag, true);
    picker.addEventListener('change', () => (picker.dataset.picked = '1'));
    ms.addEventListener('keydown', (e) => e.key === 'Enter' && setLag());
    legend.addEventListener('click', (e) => {
      const row = (e.target as HTMLElement).closest<HTMLElement>('[data-group]');
      if (!row) return;
      const group = Number(row.dataset.group);
      picker.value = String(group);
      picker.dataset.picked = '1';
      const current = this.lagOf(group);
      if (current) ms.value = String(current);
    });
    this.els.lagPicker = picker;

    make('div', r, 'title', 'Spatial Layer Cells').style.marginTop = '10px';
    checkbox('spatial', 'Show their areas', 'L', true);
    const cellsRow = make('div', r, 'row', 'In use ');
    button(cellsRow, '-', 'Use one fewer Spatial Layer Cell', () =>
      this.assignment && debug({ set_spatial: Math.max(1, this.assignment.spatial.length - 1) }),
    );
    this.els.spatialCount = make('b', cellsRow, '', '?');
    button(cellsRow, '+', 'Use one more Spatial Layer Cell', () =>
      this.assignment && debug({ set_spatial: Math.min(this.assignment.spatial_running, this.assignment.spatial.length + 1) }),
    );
    this.els.spatialRunning = make('span', cellsRow, 'muted');
    this.els.spatialLegend = make('div', r, 'legend wm-spatial-legend');
    checkbox('senses', 'Show spatial queries', 'N', false);
    checkbox('view', 'Show my view on a map', 'V', false);
    const minimap = make('canvas', r, 'wm-minimap');
    minimap.title = "The whole world: Spatial Layer Cell areas, your view and what's in it, by group";
    checkbox('bus', 'Show bus traffic', 'B', false);

    make('div', r, 'title', 'Network').style.marginTop = '10px';
    checkbox('compact', 'Optimized network traffic', 'T', this.client.settings.compact,
      "Binary snapshots with only what changed, and debug traffic only while you're looking at it");
    muted('net');
    checkbox('binaryBus', 'Optimized internal traffic', 'I', true,
      'Messages between server processes as MessagePack instead of JSON. Applies to every process at once');
    muted('bus-rate');

    make('div', r, 'title', 'Prediction').style.marginTop = '10px';
    checkbox('predict', 'Client prediction', 'P', this.client.predictor?.enabled ?? false);
    checkbox('ghosts', 'Show raw server position', 'G', false);
    const select = (label: string, options: [number, string][], after: string, onChange: (v: number) => void) => {
      const row = make('div', r, 'row', `${label} `);
      const s = make('select', row);
      s.innerHTML = options.map(([v, text]) => `<option value="${v}">${text}</option>`).join('');
      if (after) row.append(` ${after}`);
      s.addEventListener('change', () => {
        onChange(Number(s.value));
        s.blur();
      });
    };
    select('Fake latency', [[0, '0 ms'], [50, '50 ms'], [100, '100 ms'], [200, '200 ms']], 'each way', (v) => (this.client.fakeLatency = v));
    select('Jitter', [[0, 'none'], [25, 'up to 25 ms'], [50, 'up to 50 ms'], [100, 'up to 100 ms']], '', (v) => (this.client.fakeJitter = v));
    muted('prediction');

    make('div', r, 'title', 'Metrics').style.marginTop = '10px';
    checkbox('metrics', 'Show CPU, memory, traffic', 'M', false);
    return minimap;
  }

  private onKey(e: KeyboardEvent) {
    if (e.repeat || e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
    const keys: Record<string, Parameters<DebugPanel['set']>[0]> = {
      KeyT: 'compact', KeyI: 'binaryBus', KeyO: 'groups', KeyL: 'spatial', KeyN: 'senses',
      KeyV: 'view', KeyP: 'predict', KeyG: 'ghosts', KeyM: 'metrics', KeyB: 'bus',
    };
    const name = keys[e.code];
    if (!name) return;
    const current =
      name === 'compact' ? this.client.settings.compact
      : name === 'binaryBus' ? (this.assignment?.binary_bus ?? false)
      : name === 'predict' ? (this.client.predictor?.enabled ?? false)
      : name === 'metrics' ? this.metrics.visible
      : name === 'bus' ? this.bus.visible
      : this.overlays[name];
    this.set(name, !current);
  }

  private setHtml(el: HTMLElement, html: string) {
    if (el.innerHTML !== html) el.innerHTML = html;
  }

  private applyAssignment(a: Assignment) {
    this.assignment = a;
    this.bus.setProcesses(a.groups, a.spatial);
    this.bus.onOwnership();
    const count = this.els.groupsCount as HTMLInputElement;
    if (document.activeElement !== count) count.value = String(a.groups.length);
    this.els['groups-running'].textContent =
      `${a.groups_running} running on ${a.group_hosts} process${a.group_hosts === 1 ? '' : 'es'}, no limit (type any number)`;
    this.els.spatialCount.textContent = String(a.spatial.length);
    this.els.spatialRunning.textContent = `of ${a.spatial_running} running`;
    const box = this.boxes.get('binaryBus');
    if (box) box.checked = a.binary_bus;
    this.updateLegends();
  }

  private applyStats(processes: ProcessStats[]) {
    this.latestStats = { at: performance.now(), processes };
    this.bus.onStats(processes);
    this.processStats.clear();
    for (const p of processes) if (p.role === 'group' || p.role === 'spatial') this.processStats.set(p.id, p);
    this.updateLegends();
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private applySnapshot(s: Snapshot<any, any>) {
    const notes: TraceNote[] = [];
    if (this.options.traceNote) {
      for (const ev of s.events) {
        const n = this.options.traceNote(ev);
        if (n) notes.push(n);
      }
    }
    this.bus.onSnapshot(s.bus, s.streams, notes);
  }

  private updateLegends() {
    const a = this.assignment;
    if (!a) return;
    const mine = this.client.entities.get(this.client.me)?.state.group;
    this.setHtml(
      this.els.groupLegend,
      a.groups
        .map((id) => {
          const s = this.processStats.get(id);
          const lag = this.lagOf(id);
          return (
            `<div class="item" data-group="${id}" title="${this.groupName(id)}: click to pick it for fake lag">` +
            `<span class="swatch" style="background:${css(this.groupColor(id))}"></span>${this.short(id)}${id === mine ? ' (you)' : ''}` +
            `<span class="muted">${s ? `${s.players} entities` : ''}</span></div>` +
            (lag ? `<div class="sub lag" data-group="${id}">fake lag ${lag} ms each way</div>` : '')
          );
        })
        .join(''),
    );
    // The fake lag picker lists the groups in use, keeping the current choice.
    const picker = this.els.lagPicker as HTMLSelectElement;
    const options = a.groups.map((id) => `<option value="${id}">${this.short(id)}${id === mine ? ' (you)' : ''}</option>`).join('');
    if (picker.dataset.options !== options) {
      const keep = picker.value;
      picker.innerHTML = options;
      picker.dataset.options = options;
      if (a.groups.some((id) => String(id) === keep)) picker.value = keep;
    }
    // Your own group until you pick one.
    if (!picker.dataset.picked && mine !== undefined && a.groups.includes(mine)) picker.value = String(mine);
    this.setHtml(
      this.els.spatialLegend,
      a.spatial
        .map((id) => {
          const s = this.processStats.get(id);
          const sub = s ? `<div class="sub">${s.viewers} watching · ${Math.round(s.queries)} questions/s</div>` : '';
          return `<div class="item" title="${this.spatialName(id)}"><span class="swatch square" style="background:#c3c2b7"></span>${this.short(id)}<span class="muted">${s ? `${s.players} held` : ''}</span></div>${sub}`;
        })
        .join(''),
    );
  }

  private updatePrediction() {
    const p = this.client.predictor;
    if (!p) return;
    const e = p.stats.early;
    const arrival = Number.isNaN(e) ? '-' : e >= 0 ? `${Math.round(e)} ms early` : `${Math.round(-e)} ms late`;
    this.setHtml(
      this.els.prediction,
      `running ${Math.round(p.lead * this.client.tickMs)} ms ahead · last input arrived ${arrival}<br>` +
        `corrections ${p.stats.corrections} (last ${p.stats.lastError.toFixed(3)} tiles)`,
    );
  }

  private everySecond() {
    const now = performance.now();
    const m = this.client.takeMeter();
    const frames = this.frames;
    this.frames = { count: 0, busyMs: 0 };
    const heap = (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory;
    const sample: ClientSample = {
      cpuPct: ((m.busyMs + frames.busyMs) / 1000) * 100,
      heapMb: heap ? heap.usedJSHeapSize / 1024 / 1024 : null,
      down: { msgs: m.downMsgs, bytes: m.downBytes },
      up: { msgs: m.upMsgs, bytes: m.upBytes },
      fps: frames.count,
    };
    const compact = this.client.settings.compact;
    this.els.net.textContent = `receiving ${Math.round(sample.down.bytes / 1024)} KB/s (${compact ? 'optimized' : 'plain JSON'})`;
    const fresh = this.latestStats && now - this.latestStats.at < 2500 ? this.latestStats.processes : undefined;
    if (fresh) {
      const busBytes = fresh.filter((p) => p.role !== 'nats').reduce((sum, p) => sum + p.bus_out.bytes, 0);
      const rate = busBytes >= 1024 * 1024 ? `${(busBytes / 1024 / 1024).toFixed(1)} MB/s` : `${Math.round(busBytes / 1024)} KB/s`;
      this.els['bus-rate'].textContent = `servers publishing ${rate} (${this.assignment?.binary_bus ? 'binary' : 'JSON'})`;
    }
    this.metrics.push(fresh, sample);
  }
}
