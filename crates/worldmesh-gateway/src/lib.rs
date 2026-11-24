//! Gateway: holds players' WebSockets.
//!
//! - A player's inputs and commands go to their entity (`entity.<id>`): the bus delivers them
//!   to whichever Server Entity Group simulates it, wherever that is. They arrive as JSON, are
//!   checked against the world's input and command types, and travel on as MessagePack.
//! - Every tick it tells the Spatial Layer Cells where each player is looking, and puts each
//!   player's snapshot together from what those cells send back. A player's view can cover the
//!   areas of several cells; the gateway merges their parts.
//! - Joins go to the orchestrator (it picks the group); leaves go to every group.
//!
//! The gateway runs no game code but the world's join function. Components and events pass
//! through as bytes (decoded to JSON, by name, only for players who asked for JSON snapshots).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc, Notify};
use tracing::{debug, info, warn};

use crate::snapshot::Encoder;
use worldmesh_core::config::WorldConfig;
use worldmesh_core::protocol::{
    Assignment, ClientMsg, DebugCommand, DebugReport, EntityId, GroupCommand, GroupCommandKind,
    JoinRequest, Kind, PlayerIdRequest, PlayerIds, ProcessStats, SenseDebug, ServerMsg,
    SnapshotEntity, SpatialViews, StreamKind, StreamMsg, ViewEntity, Viewer, Viewers, WireEvent,
};
use worldmesh_core::subjects::{
    entity_subject, gateway_ids_subject, gateway_views_subject, random_gateway_id, ASSIGNMENTS,
    DEBUG_COMMANDS, DEBUG_REPORTS, GROUP_COMMANDS, JOINS, PLAYER_IDS, STATS, VIEWERS,
};
use worldmesh_sim::world::Rules;
pub mod snapshot;

use worldmesh_core::views::{from_player, merge_view};
use worldmesh_core::{bus, codec, stats, time};

/// Longest to wait after a tick boundary for every Spatial Layer Cell's views (they wait up
/// to 40 ms for the groups first). Normally everything arrives within a few ms.
const MERGE_TIMEOUT: Duration = Duration::from_millis(60);
/// A player missing from their own view for this many 1 s checks is re-joined.
const REJOIN_AFTER_CHECKS: u32 = 3;
/// Inputs may be scheduled at most this far ahead.
const MAX_INPUT_LEAD_MS: u64 = 5000;
/// With fewer player ids than this left, ask the orchestrator for more...
const IDS_LOW: usize = 16;
/// ...this many at a time.
const IDS_BLOCK: u32 = 32;
/// How long a connecting player waits for an id (only without an orchestrator) before the
/// gateway gives up on them.
const ID_WAIT: Duration = Duration::from_secs(5);

/// Player ids from the orchestrator, waiting for players to connect. Every gateway gets its
/// own, so any number of gateways can run at once without two giving out the same id.
#[derive(Default)]
struct IdPool {
    ids: Mutex<VecDeque<EntityId>>,
    /// The highest id we've been given. We tell the orchestrator, so even a restarted one
    /// never gives anyone those again.
    highest: AtomicU32,
    arrived: Notify,
    /// Running low: ask now, not at the next once-a-second round.
    low: Notify,
}

impl IdPool {
    fn add(&self, ids: PlayerIds) {
        if ids.count == 0 {
            return;
        }
        let last = ids.first.saturating_add(ids.count - 1);
        self.ids.lock().unwrap().extend(ids.first..=last);
        self.highest.fetch_max(last, Ordering::Relaxed);
        self.arrived.notify_waiters();
    }

    /// How many to ask for: a block when running low, otherwise none.
    fn wanted(&self) -> u32 {
        if self.ids.lock().unwrap().len() < IDS_LOW {
            IDS_BLOCK
        } else {
            0
        }
    }

    /// The next id for a connecting player, waiting for one if need be.
    async fn take(&self) -> Option<EntityId> {
        let deadline = tokio::time::Instant::now() + ID_WAIT;
        loop {
            let arrived = self.arrived.notified();
            tokio::pin!(arrived);
            arrived.as_mut().enable();
            let (id, left) = {
                let mut ids = self.ids.lock().unwrap();
                (ids.pop_front(), ids.len())
            };
            if left < IDS_LOW {
                self.low.notify_one();
            }
            if id.is_some() {
                return id;
            }
            if tokio::time::timeout_at(deadline, arrived).await.is_err() {
                return None;
            }
        }
    }
}

/// How a gateway runs.
#[derive(Debug, Clone)]
pub struct GatewayOptions {
    /// Address to listen on, like `0.0.0.0:8080`. Players connect to `/ws`.
    pub listen: String,
    /// NATS monitoring address (`host:8222`), to include the bus's own numbers in the stats.
    pub nats_monitor: Option<String>,
    /// Accept debug panel commands (scaling, fake lag, bus encoding) from any connected
    /// player. For development only.
    pub allow_debug: bool,
}

impl Default for GatewayOptions {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".into(),
            nats_monitor: None,
            allow_debug: false,
        }
    }
}

/// A connected player: where to send their snapshots, and where their view is centred.
struct Client {
    tx: mpsc::UnboundedSender<Bytes>,
    at: (f32, f32),
    /// Whether their own entity was in their last view.
    present: bool,
    /// Optimized snapshots (binary, only what changed), and whether to include debug traffic.
    compact: bool,
    debug: bool,
    encoder: Encoder,
}

#[derive(Clone)]
struct AppState {
    nats: async_nats::Client,
    rules: Arc<Rules>,
    config: WorldConfig,
    options: GatewayOptions,
    /// Encoded messages for every player (assignment changes and stats).
    out: broadcast::Sender<Bytes>,
    /// This gateway's id: random, so any number can run.
    gateway: u32,
    ids: Arc<IdPool>,
    /// Latest encoded assignment, sent to players as they connect.
    ownership: Arc<Mutex<Option<Bytes>>>,
    clients: Arc<Mutex<BTreeMap<EntityId, Client>>>,
}

/// A gateway process.
pub async fn run(
    nats: async_nats::Client,
    rules: Arc<Rules>,
    options: GatewayOptions,
) -> Result<()> {
    let config = rules.config;
    let gateway_id = random_gateway_id();
    let (out, _) = broadcast::channel::<Bytes>(64);
    let ownership = Arc::new(Mutex::new(None::<Bytes>));
    let clients: Arc<Mutex<BTreeMap<EntityId, Client>>> = Arc::default();

    // Each Spatial Layer Cell's latest views; wake the merge loop when some arrive.
    let latest: Arc<Mutex<BTreeMap<u32, SpatialViews>>> = Arc::default();
    let arrived = Arc::new(Notify::new());
    let mut views =
        bus::subscribe::<SpatialViews>(&nats, &gateway_views_subject(gateway_id)).await?;
    {
        let (latest, arrived) = (latest.clone(), arrived.clone());
        tokio::spawn(async move {
            while let Some(v) = views.recv().await {
                latest.lock().unwrap().insert(v.spatial, v);
                arrived.notify_one();
            }
        });
    }

    // Player ids: whatever the orchestrator sends us, and once a second (or when running low)
    // what we want and the highest we hold.
    let ids = Arc::new(IdPool::default());
    let mut ids_in = bus::subscribe::<PlayerIds>(&nats, &gateway_ids_subject(gateway_id)).await?;
    {
        let ids = ids.clone();
        tokio::spawn(async move {
            while let Some(block) = ids_in.recv().await {
                ids.add(block);
            }
        });
    }
    {
        let (ids, nats) = (ids.clone(), nats.clone());
        tokio::spawn(async move {
            let mut every_second = tokio::time::interval(Duration::from_secs(1));
            let mut asked: Option<Instant> = None;
            loop {
                let low = tokio::select! {
                    _ = every_second.tick() => false,
                    _ = ids.low.notified() => true,
                };
                let count = ids.wanted();
                // When running low, ask at once, but not again before the answer is back.
                if low
                    && (count == 0
                        || asked.is_some_and(|t| t.elapsed() < Duration::from_millis(500)))
                {
                    continue;
                }
                if count > 0 {
                    asked = Some(Instant::now());
                }
                let request = PlayerIdRequest {
                    gateway: gateway_id,
                    count,
                    above: ids.highest.load(Ordering::Relaxed),
                };
                bus::publish(&nats, PLAYER_IDS.to_string(), &request).await;
            }
        });
    }

    // Debug reports from every process, collected until the next merged tick.
    let reports: Arc<Mutex<Vec<DebugReport>>> = Arc::default();
    let mut debug_in = bus::subscribe::<DebugReport>(&nats, DEBUG_REPORTS).await?;
    {
        let reports = reports.clone();
        tokio::spawn(async move {
            while let Some(r) = debug_in.recv().await {
                let mut list = reports.lock().unwrap();
                list.push(r);
                if list.len() > 2000 {
                    list.drain(..1000); // nobody is collecting (no players): don't grow forever
                }
            }
        });
    }

    // Forward assignment changes to every player; remember the Spatial Layer Cells in use.
    let in_use: Arc<Mutex<Vec<u32>>> = Arc::default();
    let mut assignments = bus::subscribe::<Assignment>(&nats, ASSIGNMENTS).await?;
    {
        let (ownership, out, in_use) = (ownership.clone(), out.clone(), in_use.clone());
        tokio::spawn(async move {
            while let Some(assignment) = assignments.recv().await {
                bus::set_binary(assignment.binary_bus);
                *in_use.lock().unwrap() = assignment.spatial.clone();
                let Ok(encoded) = serde_json::to_vec(&ServerMsg::Ownership(assignment)) else {
                    continue;
                };
                let encoded = Bytes::from(encoded);
                let mut cached = ownership.lock().unwrap();
                if cached.as_ref() != Some(&encoded) {
                    *cached = Some(encoded.clone());
                    let _ = out.send(encoded);
                }
            }
        });
    }

    // Every process's latest stats; once a second, add NATS's own and send them all to players.
    stats::spawn_reporter(nats.clone(), "gateway", gateway_id);
    let mut stats_in = bus::subscribe::<ProcessStats>(&nats, STATS).await?;
    {
        let out = out.clone();
        let nats_monitor = options.nats_monitor.clone();
        tokio::spawn(async move {
            let mut latest: BTreeMap<(String, u32), (Instant, ProcessStats)> = BTreeMap::new();
            let mut nats_monitor = nats_monitor.map(stats::NatsMonitor::new);
            let mut every_second = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    Some(s) = stats_in.recv() => {
                        latest.insert((s.role.clone(), s.id), (Instant::now(), s));
                    }
                    _ = every_second.tick() => {
                        latest.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(3));
                        let mut processes: Vec<ProcessStats> = latest.values().map(|(_, s)| s.clone()).collect();
                        if let Some(nats_stats) = match nats_monitor.as_mut() {
                            Some(monitor) => monitor.sample().await,
                            None => None,
                        } {
                            processes.push(nats_stats);
                        }
                        if let Ok(encoded) = serde_json::to_vec(&ServerMsg::Stats { processes }) {
                            let _ = out.send(Bytes::from(encoded));
                        }
                    }
                }
            }
        });
    }

    // Once per tick: tell the Spatial Layer Cells where everyone is looking, then as soon as
    // every reporting cell has sent its views for this tick (or after MERGE_TIMEOUT), put each
    // player's snapshot together and send it.
    {
        let (latest, clients, reports, in_use, nats) = (
            latest.clone(),
            clients.clone(),
            reports.clone(),
            in_use.clone(),
            nats.clone(),
        );
        let rules = rules.clone();
        tokio::spawn(async move {
            // Last tick each cell's events were forwarded for, so none goes out twice.
            let mut forwarded: BTreeMap<u32, u64> = BTreeMap::new();
            loop {
                tokio::time::sleep(time::until_next_tick()).await;
                let tick = time::current_tick();
                let viewers: Vec<Viewer> = clients
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(&player, c)| Viewer {
                        player,
                        x: c.at.0,
                        y: c.at.1,
                    })
                    .collect();
                bus::publish(
                    &nats,
                    VIEWERS.to_string(),
                    &Viewers {
                        gateway: gateway_id,
                        tick,
                        viewers: viewers.clone(),
                    },
                )
                .await;
                let mut own_streams: Vec<StreamMsg> = in_use
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|&s| StreamMsg {
                        from: 0,
                        to: s,
                        kind: StreamKind::Viewers,
                        n: viewers.len() as u32,
                    })
                    .collect();

                let deadline = tokio::time::Instant::now() + MERGE_TIMEOUT;
                loop {
                    let ready = {
                        let latest = latest.lock().unwrap();
                        let recent: Vec<&SpatialViews> = latest
                            .values()
                            .filter(|v| v.tick + time::ticks_in(1000) >= tick)
                            .collect();
                        !recent.is_empty() && recent.iter().all(|v| v.tick >= tick)
                    };
                    if ready {
                        break;
                    }
                    if tokio::time::timeout_at(deadline, arrived.notified())
                        .await
                        .is_err()
                    {
                        let behind: Vec<String> = latest
                            .lock()
                            .unwrap()
                            .values()
                            .filter(|v| v.tick < tick)
                            .map(|v| format!("{:04x}@{}", v.spatial, v.tick))
                            .collect();
                        debug!(tick, ?behind, "stopped waiting for views");
                        break;
                    }
                }

                let (mut bus, mut streams) = (Vec::new(), Vec::new());
                for r in reports.lock().unwrap().drain(..) {
                    bus.extend(r.bus);
                    streams.extend(r.streams);
                }
                streams.append(&mut own_streams);

                let fresh: Vec<SpatialViews> = {
                    let mut latest = latest.lock().unwrap();
                    latest.retain(|_, v| v.tick + time::ticks_in(2000) >= tick); // forget cells silent for 2 s
                    latest
                        .values()
                        .filter(|v| v.tick + 2 >= tick)
                        .cloned()
                        .collect()
                };
                // Events are forwarded once per cell update, never twice.
                let new_events: BTreeSet<u32> = fresh
                    .iter()
                    .filter(|v| forwarded.get(&v.spatial).is_none_or(|&t| v.tick > t))
                    .map(|v| v.spatial)
                    .collect();
                for v in &fresh {
                    forwarded.insert(v.spatial, v.tick);
                }

                let mut clients = clients.lock().unwrap();
                for (&player, client) in clients.iter_mut() {
                    let (entities, events, senses) = merge_view(player, &fresh, &new_events);
                    let me = entities.iter().find(|e| e.id == player);
                    client.present = me.is_some();
                    if let Some(me) = me {
                        client.at = (me.x, me.y);
                    }
                    // Debug traffic only for players looking at it.
                    let (senses, bus, streams) = if client.debug {
                        (&senses[..], &bus[..], &streams[..])
                    } else {
                        (&[][..], &[][..], &[][..])
                    };
                    let encoded = if client.compact {
                        client
                            .encoder
                            .encode(player, tick, &entities, &events, senses, bus, streams)
                    } else {
                        json_snapshot(&rules, tick, &entities, &events, senses, bus, streams)
                    };
                    let _ = client.tx.send(Bytes::from(encoded));
                }
            }
        });
    }

    let state = AppState {
        nats,
        rules,
        config,
        options: options.clone(),
        out,
        gateway: gateway_id,
        ids,
        ownership,
        clients,
    };

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "ok" }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&options.listen).await?;
    info!(gateway = format!("{gateway_id:08x}"), listen = %options.listen, "gateway listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// A snapshot as JSON: each entity's kind and components, and each event, by name, decoded
/// from MessagePack.
fn json_snapshot(
    rules: &Rules,
    tick: u64,
    entities: &[ViewEntity],
    events: &[WireEvent],
    senses: &[SenseDebug],
    bus: &[worldmesh_core::protocol::BusMsg],
    streams: &[StreamMsg],
) -> Vec<u8> {
    let entities = entities
        .iter()
        .map(|e| SnapshotEntity {
            id: e.id,
            x: e.x,
            y: e.y,
            group: e.group,
            spatial: e.spatial,
            player: e.player,
            kind: rules.kind_name(e.kind).to_string(),
            view: e
                .parts
                .iter()
                .filter_map(|(c, bytes)| {
                    Some((
                        rules.component(*c)?.name.to_string(),
                        codec::bytes_to_json(bytes),
                    ))
                })
                .collect::<serde_json::Map<_, _>>()
                .into(),
        })
        .collect();
    let events = events
        .iter()
        .filter_map(|e| Some(serde_json::json!({ "name": rules.message(e.msg)?.name, "data": codec::bytes_to_json(&e.body) })))
        .collect();
    let msg = ServerMsg::Snapshot {
        tick,
        entities,
        events,
        senses: senses.to_vec(),
        bus: bus.to_vec(),
        streams: streams.to_vec(),
    };
    serde_json::to_vec(&msg).unwrap_or_default()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Where and as what the world's join function puts a joining player, as a join request for
/// the orchestrator.
fn join_request(rules: &Rules, gateway: u32, id: EntityId) -> Option<JoinRequest> {
    let spawn = rules.join(id);
    let (kind, parts) = rules.encode_spawn(&spawn)?;
    Some(JoinRequest {
        id,
        gateway,
        x: spawn.x,
        y: spawn.y,
        kind,
        parts,
    })
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let Some(id) = state.ids.take().await else {
        warn!("no player ids from the orchestrator for 5 s: is it running? Turning a player away");
        return;
    };
    let (mut sender, mut receiver) = socket.split();

    let welcome = ServerMsg::Welcome {
        id,
        tick_ms: time::tick_ms(),
        world: state.config,
        pos_scale: state.config.pos_scale(),
        game: state.rules.welcome().clone(),
        schema: state.rules.schema(),
    };
    let Ok(welcome) = serde_json::to_vec(&welcome) else {
        return;
    };
    stats::WS_OUT.add(welcome.len());
    if sender.send(Message::Binary(welcome.into())).await.is_err() {
        return;
    }
    let ownership = state.ownership.lock().unwrap().clone();
    if let Some(ownership) = ownership {
        stats::WS_OUT.add(ownership.len());
        if sender.send(Message::Binary(ownership)).await.is_err() {
            return;
        }
    }

    let Some(join) = join_request(&state.rules, state.gateway, id) else {
        warn!(id, "the world's join function gave an unknown kind, or a component that couldn't be encoded");
        return;
    };
    let mut broadcasts = state.out.subscribe();
    let (tx, mut snapshots) = mpsc::unbounded_channel::<Bytes>();
    // Plain JSON until the client says otherwise (it sends its settings as soon as it connects).
    let client = Client {
        tx,
        at: (join.x, join.y),
        present: false,
        compact: false,
        debug: false,
        encoder: Encoder::new(state.config.pos_scale()),
    };
    state.clients.lock().unwrap().insert(id, client);
    bus::publish(&state.nats, JOINS.to_string(), &join).await;
    info!(id, "player connected");
    stats::PLAYERS.fetch_add(1, Ordering::Relaxed);

    // Safety net: if a player vanishes (e.g. their group's process died), re-join them.
    let mut check = tokio::time::interval(Duration::from_secs(1));
    let mut missing_checks = 0u32;

    loop {
        let outgoing = tokio::select! {
            incoming = receiver.next() => {
                let reply = match incoming {
                    Some(Ok(Message::Text(text))) => {
                        stats::WS_IN.add(text.len());
                        handle_client_msg(&state, id, text.as_bytes()).await
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        stats::WS_IN.add(bytes.len());
                        handle_client_msg(&state, id, &bytes).await
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => None,
                };
                reply.and_then(|r| serde_json::to_vec(&r).ok()).map(Bytes::from)
            },
            Some(bytes) = snapshots.recv() => Some(bytes),
            update = broadcasts.recv() => match update {
                Ok(bytes) => Some(bytes),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(id, skipped, "client lagging");
                    None
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = check.tick() => {
                let present = state.clients.lock().unwrap().get(&id).is_some_and(|c| c.present);
                missing_checks = if present { 0 } else { missing_checks + 1 };
                if missing_checks >= REJOIN_AFTER_CHECKS {
                    warn!(id, "player missing from their own view, re-joining");
                    if let Some(join) = join_request(&state.rules, state.gateway, id) {
                        bus::publish(&state.nats, JOINS.to_string(), &join).await;
                    }
                    missing_checks = 0;
                }
                None
            }
        };
        if let Some(bytes) = outgoing {
            stats::WS_OUT.add(bytes.len());
            if sender.send(Message::Binary(bytes)).await.is_err() {
                break;
            }
        }
    }

    state.clients.lock().unwrap().remove(&id);
    stats::PLAYERS.fetch_sub(1, Ordering::Relaxed);
    let leave = GroupCommand {
        to: None,
        kind: GroupCommandKind::Leave { id },
    };
    bus::publish(&state.nats, GROUP_COMMANDS.to_string(), &leave).await;
    info!(id, "player disconnected");
}

/// When an input for (`tick`, `sub`) should take effect. Groups have already simulated tick
/// `now`; `now + 1` is the earliest tick an input can still change. Tick 0 (or anything
/// already past) means "as soon as possible"; nothing is scheduled more than 5 s ahead.
fn schedule_input(tick: u64, sub: u8, now: u64) -> (u64, u8) {
    if tick <= now {
        (now + 1, 0)
    } else {
        (tick.min(now + time::ticks_in(MAX_INPUT_LEAD_MS)), sub)
    }
}

/// How early an input for (`tick`, `sub`) reached us. The change happens `sub` steps into
/// tick `tick`'s movement, which covers the time from tick - 1 to tick.
fn early_ms(tick: u64, sub: u8, substeps: u8, now_ms: u64) -> i64 {
    let tick_ms = time::tick_ms();
    let change_ms =
        tick.saturating_sub(1) * tick_ms + u64::from(sub) * tick_ms / u64::from(substeps.max(1));
    change_ms as i64 - now_ms as i64
}

/// Handle one message from a player; returns a reply for that player only, if any.
async fn handle_client_msg(state: &AppState, id: EntityId, raw: &[u8]) -> Option<ServerMsg> {
    match serde_json::from_slice::<ClientMsg>(raw) {
        Ok(ClientMsg::Input { tick, sub, input }) => {
            let input = match state.rules.input_from_json(input) {
                Ok(bytes) => bytes,
                Err(err) => {
                    warn!(id, %err, "dropping an input the game doesn't understand");
                    return None;
                }
            };
            let substeps = state.config.input_substeps.max(1);
            let sub = sub.min(substeps - 1);
            let now_ms = time::world_ms();
            let (at_tick, at_sub) = schedule_input(tick, sub, now_ms / time::tick_ms());
            let env = from_player(
                id,
                Kind::Input {
                    at_tick,
                    sub: at_sub,
                    input,
                },
                false,
            );
            bus::publish(&state.nats, entity_subject(id), &env).await;
            // Tick 0 ("as soon as possible") has no moment to be early for.
            let early = if tick == 0 {
                0
            } else {
                early_ms(tick, sub, substeps, now_ms)
            };
            Some(ServerMsg::InputAck {
                tick,
                sub,
                early_ms: early,
            })
        }
        Ok(ClientMsg::Command { name, command }) => {
            // Addressed to the player's own entity: the bus delivers it to whichever group
            // simulates them now, and the world's handlers decide what it does.
            let (msg, body) = match state.rules.command_from_json(&name, command) {
                Ok(encoded) => encoded,
                Err(err) => {
                    warn!(id, %name, %err, "dropping a command the world doesn't understand");
                    return None;
                }
            };
            let env = from_player(id, Kind::Command { msg, body }, true);
            bus::publish(&state.nats, entity_subject(id), &env).await;
            None
        }
        Ok(ClientMsg::Settings { compact, debug }) => {
            if let Some(c) = state.clients.lock().unwrap().get_mut(&id) {
                if c.compact != compact {
                    c.encoder.reset(); // the next snapshot sends everyone in full
                }
                (c.compact, c.debug) = (compact, debug);
            }
            None
        }
        Ok(ClientMsg::Debug {
            set_groups,
            set_spatial,
            regroup,
            binary_bus,
            set_lag,
        }) => {
            if !state.options.allow_debug {
                warn!(id, "debug commands are turned off on this gateway");
                return None;
            }
            info!(
                id,
                ?set_groups,
                ?set_spatial,
                ?regroup,
                ?binary_bus,
                ?set_lag,
                "debug command"
            );
            let cmd = DebugCommand {
                set_groups,
                set_spatial,
                regroup: regroup == Some(true),
                binary_bus,
                set_lag,
            };
            bus::publish(&state.nats, DEBUG_COMMANDS.to_string(), &cmd).await;
            None
        }
        Err(err) => {
            warn!(id, %err, "dropping malformed client message");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use serde_json::Value;
    use worldmesh_core::protocol::CellView;
    use worldmesh_sim::test_game::{self, Health, Hurt};

    fn entity(id: u32, x: f32, spatial: u32) -> ViewEntity {
        ViewEntity {
            id,
            x,
            y: 1.0,
            group: 1,
            spatial,
            player: true,
            kind: 0,
            parts: Vec::new(),
        }
    }

    fn cell(
        spatial: u32,
        tick: u64,
        player: u32,
        entities: Vec<ViewEntity>,
        events: Vec<&[u8]>,
    ) -> SpatialViews {
        let events = events
            .into_iter()
            .map(|e| WireEvent {
                msg: 0,
                body: e.to_vec(),
            })
            .collect();
        SpatialViews {
            spatial,
            tick,
            views: vec![CellView {
                player,
                entities,
                events,
                senses: Vec::new(),
            }],
        }
    }

    #[test]
    fn a_view_merges_cells_keeping_the_newest_copy_of_each_entity() {
        let fresh = vec![
            cell(
                0x8001,
                100,
                7,
                vec![entity(1, 10.0, 0x8001), entity(2, 31.9, 0x8001)],
                vec![b"\xa1a"],
            ),
            cell(
                0x8002,
                101,
                7,
                vec![entity(2, 32.1, 0x8002), entity(3, 40.0, 0x8002)],
                vec![b"\xa1b"],
            ),
        ];
        let (entities, events, _) = merge_view(7, &fresh, &[0x8001, 0x8002].into());
        let got: Vec<(u32, f32)> = entities.iter().map(|e| (e.id, e.x)).collect();
        assert_eq!(
            got,
            vec![(1, 10.0), (2, 32.1), (3, 40.0)],
            "2 crossed over: the newer report wins"
        );
        assert_eq!(events.len(), 2);
        let (_, events, _) = merge_view(7, &fresh, &[0x8002].into());
        assert_eq!(
            events.len(),
            1,
            "a cell's events go out once, with its first update"
        );
        let (entities, _, _) = merge_view(8, &fresh, &BTreeSet::new());
        assert!(
            entities.is_empty(),
            "nothing for a player no cell sent a view for"
        );
    }

    #[test]
    fn json_snapshots_name_kinds_components_and_events() {
        let rules = test_game::rules();
        let health = rules.component_id::<Health>().unwrap();
        let mut e = entity(4, 2.0, 0x8001);
        e.parts = vec![(health, Bytes::from(codec::to_bytes(&Health(90)).unwrap()))];
        let event = WireEvent {
            msg: rules.message_id::<Hurt>().unwrap(),
            body: codec::to_bytes(&Hurt { id: 4, hp: 90 }).unwrap(),
        };
        let bytes = json_snapshot(&rules, 55, &[e], &[event], &[], &[], &[]);
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["t"], "snapshot");
        assert_eq!(v["entities"][0]["kind"], "unit");
        assert_eq!(v["entities"][0]["view"]["health"], 90);
        assert_eq!(v["events"][0]["name"], "hurt");
        assert_eq!(v["events"][0]["data"]["hp"], 90);
    }

    #[test]
    fn player_values_are_checked_against_the_worlds_types() {
        let rules = test_game::rules();
        let (msg, body) = rules
            .command_from_json("command", serde_json::json!({ "Hit": { "target": 2 } }))
            .unwrap();
        assert_eq!(rules.message(msg).unwrap().name, "command");
        assert!(matches!(
            codec::from_bytes::<test_game::Command>(&body),
            Some(test_game::Command::Hit { target: 2 })
        ));
        assert!(rules
            .command_from_json("command", serde_json::json!({ "Teleport": {} }))
            .is_err());
        assert!(
            rules
                .command_from_json("dance", serde_json::json!(null))
                .is_err(),
            "not a command"
        );
        assert!(
            rules
                .command_from_json("damage", serde_json::json!(5))
                .is_err(),
            "an action, not a command"
        );
        assert!(rules
            .input_from_json(serde_json::json!({ "dx": 1.0, "dy": 0.0 }))
            .is_ok());
        assert!(rules
            .input_from_json(serde_json::json!({ "dx": "east" }))
            .is_err());
    }

    #[test]
    fn a_join_uses_the_worlds_join_function() {
        let rules = test_game::rules();
        let join = join_request(&rules, 1, 3).unwrap();
        assert_eq!(
            (join.id, join.x, join.y, rules.kind_name(join.kind)),
            (3, 10.5, 10.5, "unit")
        );
    }

    #[test]
    fn inputs_are_scheduled_no_earlier_than_the_next_tick_and_not_far_ahead() {
        worldmesh_core::time::set_tick_hz(10);
        assert_eq!(schedule_input(0, 5, 1000), (1001, 0), "as soon as possible");
        assert_eq!(
            schedule_input(999, 5, 1000),
            (1001, 0),
            "too late for its tick"
        );
        assert_eq!(schedule_input(1003, 5, 1000), (1003, 5));
        assert_eq!(
            schedule_input(9999, 5, 1000),
            (1050, 5),
            "at most 5 s ahead"
        );
        // Tick 1001's movement covers 100.0 s to 100.1 s; sub 8 of 16 is 100.05 s.
        assert_eq!(early_ms(1001, 8, 16, 100_000), 50);
        assert_eq!(early_ms(1001, 0, 16, 100_020), -20);
    }
}
