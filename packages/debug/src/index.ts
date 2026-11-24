// @worldmesh/debug: a debug panel for Worldmesh worlds.
//
//   const panel = new DebugPanel({ client, onOverlay: () => redraw() });
//   panel.groupColor(entity.group) // the same colours as its legend

export { DebugPanel, GROUP_COLORS, NO_GROUP_COLOR } from './panel.ts';
export type { DebugPanelOptions, Overlay } from './panel.ts';
export { BusView } from './busview.ts';
export type { TraceNote } from './busview.ts';
export { MetricsPanel } from './metrics.ts';
export type { ClientSample, PanelInfo } from './metrics.ts';
