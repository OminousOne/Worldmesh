// The panel's styles, injected once. Dark surfaces and inks from the validated chart palette.

export const CSS = `
      .wm-debug {
  position: fixed; top: 10px; right: 10px; width: 250px; padding: 10px 12px; box-sizing: border-box;
  max-height: calc(100vh - 20px); overflow: auto;
  background: rgba(10, 12, 16, 0.85); border: 1px solid #333a44; border-radius: 8px;
  color: #e8e8e8; font: 13px/1.5 system-ui, sans-serif; user-select: none;
}
.wm-debug .title { font-weight: 600; margin-bottom: 6px; }
.wm-debug .row { display: flex; align-items: center; gap: 6px; margin: 6px 0; }
.wm-debug button {
  width: 26px; height: 24px; border: 1px solid #4a5260; border-radius: 4px;
  background: #232832; color: #fff; font-size: 15px; cursor: pointer;
}
.wm-debug button:hover { background: #2e3542; }
.wm-debug button.wide { width: auto; padding: 0 8px; font-size: 13px; }
.wm-debug select { background: #232832; color: #fff; border: 1px solid #4a5260; border-radius: 4px; }
.wm-debug kbd { font-size: 11px; padding: 0 4px; border: 1px solid #4a5260; border-radius: 3px; }
.wm-debug .muted { color: #9aa3b0; }
.wm-debug input[type='number'] {
  width: 48px; background: #232832; color: #fff; border: 1px solid #4a5260; border-radius: 4px; padding: 1px 4px;
}
.wm-combat { margin-top: 4px; max-width: 230px; }

/* Bus view: same surface and inks as the metrics panel. */
.wm-bus {
  display: none; position: fixed; right: 270px; top: 10px; box-sizing: border-box; width: 460px;
  padding: 10px 12px; background: #16191e; border: 1px solid rgba(255, 255, 255, 0.1); border-radius: 8px;
  color: #f0efec; font: 12px/1.4 system-ui, sans-serif; user-select: none;
}
.wm-bus .bus-header { font-weight: 600; }
.wm-bus .bus-sub { color: #898781; font-size: 11px; margin-bottom: 4px; }
.wm-bus canvas { display: block; }
.wm-bus .bus-legend { display: flex; flex-wrap: wrap; gap: 0 12px; font-size: 11px; margin-top: 4px; }
.wm-bus .legend-item { display: flex; align-items: center; gap: 4px; }
.wm-bus .legend-name { color: #c3c2b7; }
.wm-bus .legend-value { color: #f0efec; font-variant-numeric: tabular-nums; }
.dot-key { width: 8px; height: 8px; border-radius: 50%; flex: none; }
.wm-bus .bus-traces { white-space: pre-line; color: #c3c2b7; font-size: 11px; min-height: 30px; }
.wm-debug .legend { margin: 4px 0 2px; }
.wm-group-legend { max-height: 132px; overflow-y: auto; }
.wm-groups-count { width: 52px; }
.wm-debug .legend .item { display: flex; align-items: center; gap: 6px; font-size: 12px; }
.wm-group-legend .item { cursor: pointer; }
.wm-debug .wm-group-legend .lag { color: #e8b04a; font-weight: 600; }
.wm-lag-group { max-width: 112px; }
.wm-lag-ms { width: 48px; }
.wm-loss-pct { width: 40px; }
.wm-debug .legend .swatch { width: 10px; height: 10px; border-radius: 50%; flex: none; }
.wm-debug .legend .swatch.square { border-radius: 2px; }
.wm-debug .legend .muted { margin-left: auto; }
.wm-debug .legend .sub { color: #9aa3b0; font-size: 11px; margin: -2px 0 3px 16px; }
.wm-minimap { display: none; margin: 4px 0 2px; border-radius: 4px; background: #0e1014; }

/* Metrics panel: chart surface and inks from the validated dark palette. */
.wm-metrics {
  display: none; position: fixed; z-index: 4; left: 10px; bottom: 10px; box-sizing: border-box;
  width: min(840px, calc(100vw - 290px)); max-height: calc(100vh - 100px); overflow: auto;
  padding: 10px 12px; background: #16191e; border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 8px; color: #f0efec; font: 12px/1.4 system-ui, sans-serif; user-select: none;
}
.wm-metrics .metrics-header { color: #c3c2b7; margin-bottom: 8px; }
.wm-metrics .metrics-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 12px 18px; }
.wm-metrics .chart { min-width: 0; }
.wm-metrics .chart-title { font-weight: 600; }
.wm-metrics .chart-note { color: #898781; font-size: 11px; min-height: 15px; }
.wm-metrics canvas { display: block; }
.wm-metrics .load-now { margin-bottom: 12px; }
.wm-metrics .load-cols { display: grid; grid-template-columns: 1fr 1fr; gap: 0 18px; margin-top: 4px; }
.wm-metrics .load-title { color: #898781; font-size: 11px; margin-bottom: 2px; }
.wm-metrics .load-list { max-height: 150px; overflow-y: auto; }
.wm-metrics .load-row { display: grid; grid-template-columns: 72px 1fr 44px auto; align-items: center; gap: 6px; font-size: 11px; height: 15px; }
.wm-metrics .load-name { display: flex; align-items: center; gap: 4px; color: #c3c2b7; white-space: nowrap; }
.wm-metrics .load-name i { width: 8px; height: 8px; border-radius: 50%; flex: none; }
.wm-metrics .load-track { height: 6px; background: #23272e; border-radius: 3px; overflow: hidden; }
.wm-metrics .load-bar { display: block; height: 100%; border-radius: 0 3px 3px 0; }
.wm-metrics .load-val { color: #f0efec; text-align: right; font-variant-numeric: tabular-nums; }
.wm-metrics .load-detail { color: #898781; white-space: nowrap; }
.wm-metrics .chart-legend { display: flex; flex-wrap: wrap; gap: 0 10px; font-size: 11px; margin-top: 2px; }
.wm-metrics .legend-item { display: flex; align-items: center; gap: 4px; }
.wm-metrics .legend-name { color: #c3c2b7; }
.wm-metrics .legend-value { color: #f0efec; font-variant-numeric: tabular-nums; }
.line-key { width: 12px; height: 2px; border-radius: 1px; flex: none; }
.metrics-tooltip {
  position: fixed; z-index: 10; pointer-events: none; padding: 6px 8px; background: #23272e;
  border: 1px solid rgba(255, 255, 255, 0.1); border-radius: 6px; color: #c3c2b7;
  font: 11px/1.5 system-ui, sans-serif; white-space: nowrap;
}
.metrics-tooltip .tip-time { color: #898781; }
.metrics-tooltip .tip-row { display: flex; align-items: center; gap: 6px; }
.metrics-tooltip b { color: #f0efec; font-variant-numeric: tabular-nums; }
    
`;

let injected = false;
export function injectStyles() {
  if (injected) return;
  injected = true;
  const style = document.createElement('style');
  style.textContent = CSS;
  document.head.appendChild(style);
}
