// @worldmesh/client: connect a browser to a Worldmesh world.
//
//   const client = new WorldmeshClient<Components, Events, Welcome, Input, Commands, State>(
//     { url: 'ws://localhost:8080/ws', prediction: rules },
//     { welcome: (w) => loadMap(w.game.map) },
//   );
//   client.on('hit', (hit) => flash(hit.target));
//   client.connect();
//   client.sendCommand('use', { ability: 'bow', target: { entity: 7 } });
//   // every frame:
//   client.frame(dt);
//   for (const [id] of client.entities) draw(id, client.positionOf(id));

export { WorldmeshClient, sampleAt } from './client.ts';
export type { ClientHandlers, ClientOptions, Sample, Status, Tracked } from './client.ts';
export { Predictor } from './predictor.ts';
export type { PredictionRules, PredictorOptions, Timed } from './predictor.ts';
export { SnapshotDecoder, isCompact } from './snapshot.ts';
export * from './protocol.ts';
