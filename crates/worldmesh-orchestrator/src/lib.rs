//! Orchestrator: decides which Server Entity Groups and Spatial Layer Cells are in use, which
//! chunks each Spatial Layer Cell owns, and which group each new entity goes to (players who
//! join, and entities the game spawns, which it also gives ids). It never touches game traffic
//! and never runs game code.
//!
//! Scaling is manual, from the debug panel:
//! - **Groups**: any number. Group processes run as many groups as they're asked to: raising
//!   the count starts new groups on the least busy processes, and once they're all up,
//!   entities are re-split and the ones that move are handed off live. Lowering it hands off
//!   everyone in the groups taken out of use, and stops those groups once they've been empty
//!   for a few seconds.
//! - **Spatial Layer Cells**: changing how many are in use re-splits the chunks. Nothing is
//!   handed off; groups just send their next reports to the new owners.
//!
//! Entities are split by count, keeping neighbours together: sorted along a Hilbert curve
//! (close on the curve means close on the map), cut into equal runs, each run given to the
//! group already holding most of it, so as few entities as possible move.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{info, warn};

use worldmesh_core::config::{partition, WorldConfig};
use worldmesh_core::protocol::{
    Assignment, DebugCommand, EntityId, GroupCommand, GroupCommandKind, GroupId, Heartbeat, HostCommand, JoinRequest,
    NewEntity, SpawnRequest, SPAWNED_ID_START,
};
use worldmesh_core::subjects::{random_process_id, ASSIGNMENTS, DEBUG_COMMANDS, GROUP_COMMANDS, HEARTBEATS, HOST_COMMANDS, JOINS, SPAWNS};
use worldmesh_core::{bus, stats, time};

/// A process that hasn't sent a heartbeat for this long is considered dead.
const PROCESS_TIMEOUT: Duration = Duration::from_secs(3);
/// A release isn't repeated for the same entity within this long (the group may not have
/// reported the handoff yet).
const RESEND_MOVE_AFTER: Duration = Duration::from_secs(2);
/// Safety limit on groups in use (each one ticks every tick, even empty).
pub const MAX_GROUPS: u32 = 1000;
/// Most fake lag (debug) a group can be given, in ms.
pub const MAX_FAKE_LAG_MS: u32 = 5000;
/// A group asked for but not heard from after this long is asked for again.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// A group out of use is stopped once it has been empty this long (past the 1 s it keeps
/// forwarding strays after its last handoff).
const IDLE_BEFORE_STOP: Duration = Duration::from_secs(3);

/// Split entities between `groups`. Returns entity -> group.
pub fn plan(
    config: &WorldConfig,
    entities: &[(EntityId, f32, f32)],
    groups: &[GroupId],
    current: &BTreeMap<EntityId, GroupId>,
) -> BTreeMap<EntityId, GroupId> {
    if groups.is_empty() {
        return BTreeMap::new();
    }
    let mut sorted: Vec<(u64, EntityId)> = entities.iter().map(|&(id, x, y)| (config.tile_hilbert_index(x, y), id)).collect();
    sorted.sort_unstable();
    let (k, n) = (groups.len(), sorted.len());
    let runs: Vec<&[(u64, EntityId)]> = (0..k).map(|r| &sorted[r * n / k..(r + 1) * n / k]).collect();
    // overlap[r][g]: how many of run r group g already has.
    let overlap: Vec<Vec<usize>> = runs
        .iter()
        .map(|run| groups.iter().map(|g| run.iter().filter(|(_, id)| current.get(id) == Some(g)).count()).collect())
        .collect();
    let order = best_matching(&overlap);
    let mut out = BTreeMap::new();
    for (r, run) in runs.iter().enumerate() {
        for &(_, id) in run.iter() {
            out.insert(id, groups[order[r]]);
        }
    }
    out
}

/// Give each run a different group, keeping as many entities where they are as possible.
/// Tries every pairing for up to 7 groups; greedy beyond that.
fn best_matching(score: &[Vec<usize>]) -> Vec<usize> {
    let k = score.len();
    if k <= 7 {
        let mut best = (0, (0..k).collect::<Vec<_>>());
        let mut perm: Vec<usize> = (0..k).collect();
        permute(&mut perm, 0, &mut |p| {
            let total: usize = p.iter().enumerate().map(|(r, &g)| score[r][g]).sum();
            if total > best.0 {
                best = (total, p.to_vec());
            }
        });
        return best.1;
    }
    let mut taken = vec![false; k];
    (0..k)
        .map(|r| {
            let g = (0..k).filter(|&g| !taken[g]).max_by_key(|&g| (score[r][g], std::cmp::Reverse(g))).expect("one left");
            taken[g] = true;
            g
        })
        .collect()
}

fn permute(p: &mut Vec<usize>, i: usize, visit: &mut impl FnMut(&[usize])) {
    if i == p.len() {
        return visit(p);
    }
    for j in i..p.len() {
        p.swap(i, j);
        permute(p, i + 1, visit);
        p.swap(i, j);
    }
}

/// Up to `n` of `live`: those in `current` first, in their order, then the rest by id.
fn stable_pick(current: &[u32], live: &[u32], n: u32) -> Vec<u32> {
    let mut out: Vec<u32> = current.iter().copied().filter(|id| live.contains(id)).collect();
    out.extend(live.iter().copied().filter(|id| !current.contains(id)));
    out.truncate(n as usize);
    out
}

struct LiveGroup {
    seen: Instant,
}

struct Orchestrator {
    nats: async_nats::Client,
    config: WorldConfig,
    order: Vec<usize>,
    desired_groups: u32,
    desired_spatial: u32,
    groups: BTreeMap<u32, LiveGroup>,
    spatial: BTreeMap<u32, Instant>,
    /// Group processes: when last heard from, and the groups they run.
    hosts: BTreeMap<u32, (Instant, Vec<u32>)>,
    /// Groups asked for and not heard from yet: id -> (process, when asked).
    starting: BTreeMap<u32, (u32, Instant)>,
    /// Groups recently stopped (a late heartbeat from one is ignored).
    stopped: BTreeMap<u32, Instant>,
    /// Groups out of use with no entities: since when.
    idle_since: BTreeMap<u32, Instant>,
    current: Option<Assignment>,
    version: u64,
    /// Where each entity is, as far as we know: entity -> (group, x, y). From heartbeats, plus
    /// what we placed or moved since.
    members: BTreeMap<u32, (u32, f32, f32)>,
    /// Releases sent recently: entity -> when.
    moves_sent: BTreeMap<u32, Instant>,
    pending_joins: Vec<JoinRequest>,
    /// The next id to give an entity the game spawns.
    next_spawn_id: EntityId,
    /// Bus encoding everyone uses (debug toggle).
    binary_bus: bool,
    /// Fake network lag per group, in ms (debug).
    lag: BTreeMap<u32, u32>,
}

impl Orchestrator {
    /// Groups in use: the ones already in use keep their place (so "Group 2" stays Group 2),
    /// extra ones are added in id order, and scaling down drops the last ones.
    fn active_groups(&self) -> Vec<u32> {
        let current = self.current.as_ref().map(|c| c.groups.as_slice()).unwrap_or(&[]);
        stable_pick(current, &self.groups.keys().copied().collect::<Vec<_>>(), self.desired_groups)
    }

    fn active_spatial(&self) -> Vec<u32> {
        let current = self.current.as_ref().map(|c| c.spatial.as_slice()).unwrap_or(&[]);
        stable_pick(current, &self.spatial.keys().copied().collect::<Vec<_>>(), self.desired_spatial)
    }

    fn heartbeat(&mut self, hb: Heartbeat) {
        let now = Instant::now();
        if hb.role == "host" {
            if self.hosts.insert(hb.id, (now, hb.groups)).is_none() {
                info!(process = format!("{:04x}", hb.id), "group process registered");
            }
            return;
        }
        if hb.role == "group" && self.stopped.contains_key(&hb.id) {
            return;
        }
        if hb.role == "spatial" {
            if self.spatial.insert(hb.id, now).is_none() {
                info!(spatial = format!("{:04x}", hb.id), "Spatial Layer Cell registered");
            }
            return;
        }
        if !self.groups.contains_key(&hb.id) {
            info!(group = format!("{:04x}", hb.id), "Server Entity Group registered");
        }
        self.starting.remove(&hb.id);
        self.members.retain(|_, (g, ..)| *g != hb.id);
        for &(id, x, y) in &hb.entities {
            self.members.insert(id, (hb.id, x, y));
        }
        self.groups.insert(hb.id, LiveGroup { seen: now });
    }

    async fn send(&self, to: Option<u32>, kind: GroupCommandKind) {
        bus::publish(&self.nats, GROUP_COMMANDS.to_string(), &GroupCommand { to, kind }).await;
    }

    /// Place new entities in the groups in use (counts stay even, neighbours together); nobody
    /// already placed moves.
    async fn place(&mut self, new: Vec<NewEntity>) {
        let groups = self.active_groups();
        if groups.is_empty() || new.is_empty() {
            return;
        }
        let mut all: Vec<(u32, f32, f32)> = self.members.iter().map(|(&id, &(_, x, y))| (id, x, y)).collect();
        all.extend(new.iter().map(|n| (n.id, n.x, n.y)));
        let current: BTreeMap<u32, u32> = self.members.iter().map(|(&id, &(g, ..))| (id, g)).collect();
        let planned = plan(&self.config, &all, &groups, &current);
        let mut by_group: BTreeMap<u32, Vec<NewEntity>> = BTreeMap::new();
        for n in new {
            let g = planned[&n.id];
            self.members.insert(n.id, (g, n.x, n.y));
            by_group.entry(g).or_default().push(n);
        }
        for (g, entities) in by_group {
            info!(group = format!("{g:04x}"), count = entities.len(), "placing entities");
            self.send(Some(g), GroupCommandKind::Spawn { entities }).await;
        }
    }

    async fn place_joins(&mut self) {
        if self.active_groups().is_empty() {
            return;
        }
        let joins: Vec<NewEntity> = std::mem::take(&mut self.pending_joins)
            .into_iter()
            // The gateway re-sends a join if a player vanished; don't duplicate one that exists.
            .filter(|j| !self.members.contains_key(&j.id))
            .map(|j| NewEntity { id: j.id, x: j.x, y: j.y, player: true, kind: j.kind, parts: j.parts })
            .collect();
        self.place(joins).await;
    }

    /// Entities the game spawned: give each an id, then place them like joins.
    async fn spawn(&mut self, request: SpawnRequest) {
        let mut entities = request.entities;
        for e in &mut entities {
            loop {
                let id = self.next_spawn_id;
                self.next_spawn_id = if id == u32::MAX { SPAWNED_ID_START } else { id + 1 };
                if !self.members.contains_key(&id) {
                    e.id = id;
                    break;
                }
            }
            e.player = false;
        }
        self.place(entities).await;
    }

    /// Hand off entities to where the plan wants them. `everyone`: re-split all entities
    /// (groups changed, or regroup asked for); otherwise only those in groups out of use.
    async fn rebalance(&mut self, everyone: bool) {
        let groups = self.active_groups();
        if groups.is_empty() {
            return;
        }
        let now = Instant::now();
        let all: Vec<(u32, f32, f32)> = self.members.iter().map(|(&id, &(_, x, y))| (id, x, y)).collect();
        let current: BTreeMap<u32, u32> = self.members.iter().map(|(&id, &(g, ..))| (id, g)).collect();
        let planned = plan(&self.config, &all, &groups, &current);
        let mut by_group: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
        for (&id, &to) in &planned {
            let from = current[&id];
            let stranded = !groups.contains(&from);
            let recently = self.moves_sent.get(&id).is_some_and(|&at| now.duration_since(at) < RESEND_MOVE_AFTER);
            if from != to && (everyone || stranded) && !recently {
                by_group.entry(from).or_default().push((id, to));
            }
        }
        for (from, moves) in by_group {
            info!(group = format!("{from:04x}"), count = moves.len(), "handing entities off");
            for &(id, to) in &moves {
                self.moves_sent.insert(id, now);
                if let Some(m) = self.members.get_mut(&id) {
                    m.0 = to;
                }
            }
            self.send(Some(from), GroupCommandKind::Release { moves }).await;
        }
        self.moves_sent.retain(|_, at| now.duration_since(*at) < RESEND_MOVE_AFTER * 2);
    }

    /// Ask group processes for more groups until the count asked for is running (or starting),
    /// each new one on the process running the fewest.
    async fn ensure_groups(&mut self) {
        let now = Instant::now();
        self.starting.retain(|_, (_, at)| now.duration_since(*at) < START_TIMEOUT);
        let have = self.groups.len() + self.starting.len();
        let want = self.desired_groups as usize;
        if have >= want || self.hosts.is_empty() {
            return;
        }
        let mut load: BTreeMap<u32, usize> = self.hosts.iter().map(|(&h, (_, g))| (h, g.len())).collect();
        for (h, _) in self.starting.values() {
            *load.entry(*h).or_default() += 1;
        }
        let mut by_host: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for _ in have..want {
            let host = load.iter().min_by_key(|(&h, &n)| (n, h)).map(|(&h, _)| h).expect("hosts checked above");
            let id = loop {
                let id = random_process_id(false);
                if !self.groups.contains_key(&id) && !self.starting.contains_key(&id) && !self.stopped.contains_key(&id) {
                    break id;
                }
            };
            *load.get_mut(&host).expect("from load") += 1;
            self.starting.insert(id, (host, now));
            by_host.entry(host).or_default().push(id);
        }
        for (host, start) in by_host {
            info!(process = format!("{host:04x}"), count = start.len(), "starting groups");
            bus::publish(&self.nats, HOST_COMMANDS.to_string(), &HostCommand { host, start, stop: Vec::new() }).await;
        }
    }

    /// Stop groups out of use once they've been empty a while.
    async fn stop_idle(&mut self, in_use: &[u32]) {
        let now = Instant::now();
        let busy: BTreeSet<u32> = self.members.values().map(|&(g, ..)| g).collect();
        let mut stops: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for &g in self.groups.keys() {
            if in_use.contains(&g) || busy.contains(&g) {
                self.idle_since.remove(&g);
                continue;
            }
            let since = *self.idle_since.entry(g).or_insert(now);
            if now.duration_since(since) < IDLE_BEFORE_STOP {
                continue;
            }
            if let Some((&host, _)) = self.hosts.iter().find(|(_, (_, groups))| groups.contains(&g)) {
                stops.entry(host).or_default().push(g);
            }
        }
        for (host, stop) in stops {
            for id in &stop {
                self.groups.remove(id);
                self.idle_since.remove(id);
                self.stopped.insert(*id, now);
            }
            info!(process = format!("{host:04x}"), count = stop.len(), "stopping empty groups");
            bus::publish(&self.nats, HOST_COMMANDS.to_string(), &HostCommand { host, start: Vec::new(), stop }).await;
        }
        self.stopped.retain(|_, at| now.duration_since(*at) < Duration::from_secs(10));
    }

    fn assignment(&self, groups: Vec<u32>, spatial: Vec<u32>) -> Assignment {
        let count = self.config.chunk_count();
        let owners = if spatial.is_empty() { vec![0; count] } else { partition(&self.order, &vec![1.0; count], &spatial) };
        Assignment {
            version: self.version,
            chunk_size: self.config.chunk_size,
            chunks_x: self.config.chunks_x(),
            chunks_y: self.config.chunks_y(),
            owners,
            spatial,
            groups,
            spatial_running: self.spatial.len() as u32,
            groups_running: self.groups.len() as u32,
            group_hosts: self.hosts.len() as u32,
            binary_bus: self.binary_bus,
            group_lag: self.group_lag(),
        }
    }

    /// Lag for the groups still running, for the assignment.
    fn group_lag(&self) -> Vec<(u32, u32)> {
        self.lag.iter().filter(|(g, _)| self.groups.contains_key(g)).map(|(&g, &ms)| (g, ms)).collect()
    }
}

/// The orchestrator process: starts with `groups` groups and `spatial` Spatial Layer Cells in use.
pub async fn run(nats: async_nats::Client, config: WorldConfig, groups: u32, spatial: u32) -> Result<()> {
    stats::spawn_reporter(nats.clone(), "orchestrator", 0);
    let mut heartbeats = bus::subscribe::<Heartbeat>(&nats, HEARTBEATS).await?;
    let mut debug = bus::subscribe::<DebugCommand>(&nats, DEBUG_COMMANDS).await?;
    let mut joins = bus::subscribe::<JoinRequest>(&nats, JOINS).await?;
    let mut spawns = bus::subscribe::<SpawnRequest>(&nats, SPAWNS).await?;
    let mut o = Orchestrator {
        nats: nats.clone(),
        config,
        order: config.chunks_in_hilbert_order(),
        desired_groups: groups.max(1),
        desired_spatial: spatial.max(1),
        groups: BTreeMap::new(),
        spatial: BTreeMap::new(),
        hosts: BTreeMap::new(),
        starting: BTreeMap::new(),
        stopped: BTreeMap::new(),
        idle_since: BTreeMap::new(),
        current: None,
        version: 0,
        members: BTreeMap::new(),
        moves_sent: BTreeMap::new(),
        pending_joins: Vec::new(),
        // Start somewhere new each run, so a restarted orchestrator doesn't reuse recent ids.
        next_spawn_id: SPAWNED_ID_START + ((time::world_ms() / 100) as u32 & 0x3FFF_FFFF),
        binary_bus: true,
        lag: BTreeMap::new(),
    };
    bus::set_binary(o.binary_bus);
    let mut last_publish = Instant::now();
    let started = Instant::now();
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    info!(groups = o.desired_groups, spatial = o.desired_spatial, "orchestrator running");

    loop {
        let mut regroup = false;
        tokio::select! {
            Some(hb) = heartbeats.recv() => o.heartbeat(hb),
            Some(join) = joins.recv() => {
                o.pending_joins.push(join);
                o.place_joins().await;
            }
            Some(request) = spawns.recv() => o.spawn(request).await,
            Some(cmd) = debug.recv() => {
                if let Some(n) = cmd.set_groups {
                    o.desired_groups = n.clamp(1, MAX_GROUPS);
                    info!(groups = o.desired_groups, "debug: groups in use");
                }
                if let Some(n) = cmd.set_spatial {
                    o.desired_spatial = n.clamp(1, o.config.chunk_count() as u32);
                    info!(spatial = o.desired_spatial, "debug: Spatial Layer Cells in use");
                }
                regroup = cmd.regroup;
                if let Some((group, ms)) = cmd.set_lag {
                    let ms = ms.min(MAX_FAKE_LAG_MS);
                    if ms == 0 {
                        o.lag.remove(&group);
                    } else {
                        o.lag.insert(group, ms);
                    }
                    info!(group = format!("{group:04x}"), ms, "debug: fake lag");
                }
                if let Some(on) = cmd.binary_bus {
                    o.binary_bus = on;
                    bus::set_binary(on);
                    info!(binary = on, "debug: bus encoding");
                }
            }
            _ = interval.tick() => {}
        }

        let now = Instant::now();
        o.groups.retain(|id, g| {
            let alive = now.duration_since(g.seen) < PROCESS_TIMEOUT;
            if !alive {
                warn!(group = format!("{id:04x}"), "Server Entity Group stopped responding");
            }
            alive
        });
        let live_groups: Vec<u32> = o.groups.keys().copied().collect();
        o.members.retain(|_, (g, ..)| live_groups.contains(g)); // a dead group's entities are gone
        o.hosts.retain(|_, (seen, _)| now.duration_since(*seen) < PROCESS_TIMEOUT);
        o.spatial.retain(|id, seen| {
            let alive = now.duration_since(*seen) < PROCESS_TIMEOUT;
            if !alive {
                warn!(spatial = format!("{id:04x}"), "Spatial Layer Cell stopped responding");
            }
            alive
        });

        // Give group processes a moment to register at startup before asking for more groups
        // (each process brings its first group along).
        if started.elapsed() >= Duration::from_secs(3) {
            o.ensure_groups().await;
        }
        // While new groups are still starting, keep the groups in use as they are: re-split
        // entities once, when they're all up, rather than once per group as each arrives.
        let groups = if o.starting.is_empty() {
            o.active_groups()
        } else {
            let live = &o.groups;
            o.current.as_ref().map(|c| c.groups.iter().copied().filter(|g| live.contains_key(g)).collect()).unwrap_or_default()
        };
        let spatial = o.active_spatial();
        let changed = match &o.current {
            None => !groups.is_empty() || !spatial.is_empty(),
            Some(cur) => cur.groups != groups || cur.spatial != spatial,
        };
        let groups_changed = o.current.as_ref().is_none_or(|cur| cur.groups != groups);
        let running_changed = o.current.as_ref().is_some_and(|cur| {
            cur.groups_running != o.groups.len() as u32
                || cur.spatial_running != o.spatial.len() as u32
                || cur.group_hosts != o.hosts.len() as u32
                || cur.binary_bus != o.binary_bus
                || cur.group_lag != o.group_lag()
        });
        if changed || running_changed {
            if changed {
                o.version += 1;
                let names = |ids: &[u32]| ids.iter().map(|id| format!("{id:04x}")).collect::<Vec<_>>();
                info!(version = o.version, groups = ?names(&groups), spatial = ?names(&spatial), "new assignment");
            }
            o.current = Some(o.assignment(groups.clone(), spatial));
        }
        if let Some(cur) = &o.current {
            // Republish every second too, so processes that start later catch up.
            if changed || running_changed || now.duration_since(last_publish) >= Duration::from_secs(1) {
                bus::publish(&nats, ASSIGNMENTS.to_string(), cur).await;
                last_publish = now;
            }
        }

        o.stop_idle(&groups).await;
        o.place_joins().await;
        // Re-split everyone when the groups in use change (or on request); otherwise just make
        // sure nobody is left in a group out of use.
        o.rebalance((changed && groups_changed) || regroup).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use worldmesh_core::codec::splitmix64;

    fn config() -> WorldConfig {
        WorldConfig::default()
    }

    fn plan(entities: &[(u32, f32, f32)], groups: &[u32], current: &BTreeMap<u32, u32>) -> BTreeMap<u32, u32> {
        super::plan(&config(), entities, groups, current)
    }

    fn scatter(n: u32) -> Vec<(u32, f32, f32)> {
        (0..n).map(|i| (i, (splitmix64(u64::from(i)) % 64) as f32 + 0.5, (splitmix64(u64::from(i) + 999) % 64) as f32 + 0.5)).collect()
    }

    fn counts(p: &BTreeMap<u32, u32>, groups: &[u32]) -> Vec<usize> {
        groups.iter().map(|g| p.values().filter(|v| *v == g).count()).collect()
    }

    #[test]
    fn entities_are_split_evenly() {
        let p = plan(&scatter(90), &[1, 2, 3], &BTreeMap::new());
        assert_eq!(counts(&p, &[1, 2, 3]), vec![30, 30, 30]);
    }

    #[test]
    fn each_group_gets_a_compact_patch() {
        let entities = scatter(200);
        let p = plan(&entities, &[1, 2, 3, 4], &BTreeMap::new());
        // Average distance to the nearest same-group entity is small compared with a random split.
        let near = |same: bool| {
            let mut total = 0.0;
            for &(id, x, y) in &entities {
                let best = entities
                    .iter()
                    .filter(|o| o.0 != id && (p[&o.0] == p[&id]) == same)
                    .map(|o| ((o.1 - x).powi(2) + (o.2 - y).powi(2)).sqrt())
                    .fold(f32::MAX, f32::min);
                total += best;
            }
            total / entities.len() as f32
        };
        assert!(near(true) < 1.6 * near(false), "same-group neighbours are about as close as any neighbour");
    }

    #[test]
    fn nobody_moves_when_nothing_changed() {
        let entities = scatter(60);
        let first = plan(&entities, &[1, 2, 3], &BTreeMap::new());
        let again = plan(&entities, &[1, 2, 3], &first);
        assert_eq!(first, again);
        // Even if the group ids come in a different order.
        assert_eq!(plan(&entities, &[3, 1, 2], &first), first);
    }

    #[test]
    fn adding_a_group_moves_about_a_quarter() {
        let entities = scatter(120);
        let three = plan(&entities, &[1, 2, 3], &BTreeMap::new());
        let four = plan(&entities, &[1, 2, 3, 4], &three);
        let moved = four.iter().filter(|(id, g)| three[id] != **g).count();
        assert_eq!(counts(&four, &[1, 2, 3, 4]), vec![30, 30, 30, 30]);
        assert!(moved <= 50, "{moved} of 120 moved");
    }

    #[test]
    fn removing_a_group_rehomes_its_entities() {
        let entities = scatter(90);
        let three = plan(&entities, &[1, 2, 3], &BTreeMap::new());
        let two = plan(&entities, &[1, 2], &three);
        assert!(two.values().all(|g| *g != 3));
        assert_eq!(counts(&two, &[1, 2]), vec![45, 45]);
    }

    #[test]
    fn groups_in_use_keep_their_place() {
        assert_eq!(stable_pick(&[], &[5, 2, 9], 2), vec![5, 2]);
        assert_eq!(stable_pick(&[9, 5], &[2, 5, 9], 3), vec![9, 5, 2], "a new one goes last");
        assert_eq!(stable_pick(&[9, 5, 2], &[2, 5, 9], 2), vec![9, 5], "scaling down drops the last");
        assert_eq!(stable_pick(&[9, 5], &[2, 9], 2), vec![9, 2], "a dead one is replaced");
    }

    #[test]
    fn many_groups_use_the_greedy_matching() {
        let entities = scatter(100);
        let groups: Vec<u32> = (1..=10).collect();
        let p = plan(&entities, &groups, &BTreeMap::new());
        assert!(counts(&p, &groups).iter().all(|&c| c == 10));
        assert_eq!(plan(&entities, &groups, &p), p);
    }
}
