// @worldmesh/client: connect a browser to a Worldmesh world.
//
//   const client = new WorldmeshClient<View, Event, Welcome, Input, Command, State>(
//     { url: 'ws://localhost:8080/ws', prediction: rules },
//     { welcome: (w) => loadMap(w.game.map), snapshot: (s) => s.events.forEach(play) },
//   );
//   client.connect();
//   // every frame:
//   client.frame(dt);
//   for (const [id] of client.entities) draw(id, client.positionOf(id));

export { WorldmeshClient, sampleAt } from './client.ts';
export type { ClientHandlers, ClientOptions, Sample, Status, Tracked } from './client.ts';
export { Predictor } from './predictor.ts';
export type { PredictionRules, PredictorOptions, Timed } from './predictor.ts';
export { SnapshotDecoder, isCompact } from './snapshot.ts';
export * from './protocol.ts';
