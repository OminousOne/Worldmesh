// A world can run any number of gateways. The panel shows them as one: their numbers added up.
import type { ProcessStats, Rate } from '@worldmesh/client';

const add = (a: Rate, b: Rate): Rate => ({ msgs: a.msgs + b.msgs, bytes: a.bytes + b.bytes });
const addMaybe = (a: number | null, b: number | null) => (a == null && b == null ? null : (a ?? 0) + (b ?? 0));

/** Every gateway's numbers added up, with how many there are; undefined if there are none. */
export function gatewayTotals(processes: ProcessStats[]): (ProcessStats & { count: number }) | undefined {
  const gateways = processes.filter((p) => p.role === 'gateway');
  if (!gateways.length) return undefined;
  return gateways.slice(1).reduce<ProcessStats & { count: number }>(
    (t, g) => ({
      ...t,
      cpu_pct: addMaybe(t.cpu_pct, g.cpu_pct),
      mem_mb: addMaybe(t.mem_mb, g.mem_mb),
      bus_in: add(t.bus_in, g.bus_in),
      bus_out: add(t.bus_out, g.bus_out),
      ws_in: add(t.ws_in, g.ws_in),
      ws_out: add(t.ws_out, g.ws_out),
      players: t.players + g.players,
    }),
    { ...gateways[0], id: 0, count: gateways.length },
  );
}
