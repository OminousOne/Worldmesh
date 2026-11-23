//! Spatial Layer Cells: each owns an area (a set of chunks) and answers "what's near here?".
//!
//! A cell simulates nothing and decides nothing about entities. Every tick each Server Entity
//! Group sends it a report: that group's entities standing in this cell's area (with the
//! game's view of each, as opaque bytes). From those copies it
//!
//! - answers "what's near me?" (back to the asking group, before its next tick),
//! - passes area actions on to every group with an entity in the circle,
//! - builds each player's view of its area for the gateway.
//!
//! Everything it holds is rebuilt from the groups' reports every tick, so there is nothing to
//! hand off: when the orchestrator gives a chunk to another cell, the groups' next reports
//! simply go there. It never runs game code.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use anyhow::Result;
use tokio::time::Instant;
use tracing::{debug, info};

use worldmesh_core::config::WorldConfig;
use worldmesh_core::protocol::{
    Address, Assignment, BusKind, BusMsg, CellView, DebugReport, Envelope, EventAt, GroupId,
    GroupReport, Heartbeat, Kind, Seen, SenseAnswer, SenseAnswers, SenseDebug, SenseQuery,
    SpatialViews, StreamKind, StreamMsg, ViewEntity, Viewer, Viewers, WireEvent,
};
use worldmesh_core::subjects::{
    group_inbox_subject, group_senses_subject, random_process_id, spatial_inbox_subject,
    spatial_reports_subject, ASSIGNMENTS, DEBUG_REPORTS, HEARTBEATS, SPATIAL_ID_BIT, VIEWERS,
    VIEWS,
};
use worldmesh_core::{bus, stats, time};

/// After a tick starts, wait at most this long for every group's report before answering
/// and building views with what has arrived. Normally they all arrive within a few ms.
const WAIT_FOR_GROUPS: Duration = Duration::from_millis(40);
/// A group that hasn't reported for this long is gone; its entities are dropped.
const GROUP_TIMEOUT_MS: u64 = 1000;
/// Entities this much outside an area action's circle still get it passed on (they may have
/// moved since their last report); their group decides with exact positions.
const AREA_SLACK: f32 = 1.0;
/// Events this close outside a player's view still reach them (things at the edge of the screen).
const EVENT_MARGIN: f32 = 2.0;
/// At most this many entities listed per answers message (well under NATS's 1 MB limit). A
/// busy cell sends a group several parts per tick.
const MAX_SEEN_PER_MESSAGE: usize = 4000;

pub struct SpatialWorld {
    pub spatial_id: GroupId,
    config: WorldConfig,
    /// Spatial Layer Cell owning each chunk.
    owners: Option<Vec<GroupId>>,
    /// Whether the orchestrator is using this cell at all.
    active: bool,
    /// What each group last told us: (tick, its entities in our area).
    held: BTreeMap<GroupId, (u64, Vec<ViewEntity>)>,
    /// When each group's last report arrived (a lagging group is late, not dead).
    heard: BTreeMap<GroupId, Instant>,
    /// Questions waiting for the next answer round: (asking group, question).
    queries: Vec<(GroupId, SenseQuery)>,
    /// Questions answered in the last round, for the debug overlay in players' views.
    answered: Vec<SenseDebug>,
    /// What happened in our area, waiting to go into players' views.
    events: Vec<EventAt>,
    viewers: Vec<Viewer>,
    outbox: Vec<(String, Envelope)>,
    bus_log: Vec<BusMsg>,
}

impl SpatialWorld {
    pub fn new(spatial_id: GroupId, config: WorldConfig) -> Self {
        Self {
            spatial_id,
            config,
            owners: None,
            active: false,
            held: BTreeMap::new(),
            heard: BTreeMap::new(),
            queries: Vec::new(),
            answered: Vec::new(),
            events: Vec::new(),
            viewers: Vec::new(),
            outbox: Vec::new(),
            bus_log: Vec::new(),
        }
    }

    pub fn set_assignment(&mut self, assignment: &Assignment) {
        self.owners = Some(assignment.owners.clone());
        self.active = assignment.spatial.contains(&self.spatial_id);
        if !self.active {
            self.held.clear();
            self.heard.clear();
            self.queries.clear();
            self.events.clear();
        }
    }

    /// Entities held right now, from every group.
    pub fn entities(&self) -> impl Iterator<Item = &ViewEntity> {
        self.held.values().flat_map(|(_, list)| list.iter())
    }

    pub fn entity_count(&self) -> usize {
        self.held.values().map(|(_, list)| list.len()).sum()
    }

    /// A group's report replaces everything we had from it: an entity that walked out of our
    /// area just isn't listed any more.
    pub fn receive_report(&mut self, report: GroupReport) {
        if !self.active {
            return;
        }
        let (group, spatial) = (report.group, self.spatial_id);
        let entities: Vec<ViewEntity> = report
            .entities
            .into_iter()
            .map(|e| ViewEntity {
                id: e.id,
                x: e.x,
                y: e.y,
                group,
                spatial,
                player: e.player,
                kind: e.kind,
                parts: e.parts,
            })
            .collect();
        self.heard.insert(group, Instant::now());
        let newer = self.held.get(&group).is_none_or(|(t, _)| report.tick >= *t);
        if newer {
            self.held.insert(group, (report.tick, entities));
        }
        self.queries
            .extend(report.queries.into_iter().map(|q| (group, q)));
        self.events.extend(report.events);
    }

    pub fn receive_viewers(&mut self, viewers: Viewers) {
        self.viewers = viewers.viewers;
    }

    /// Has every group that's keeping up sent its report for `tick`? A group already more
    /// than a tick behind (lagging) isn't waited for: waiting wouldn't bring its report, and
    /// would hold up everyone else's views. Its entities show where it last said they were.
    pub fn groups_done(&self, tick: u64) -> bool {
        self.held.values().all(|(t, _)| *t >= tick || *t + 1 < tick)
    }

    /// Forget groups we haven't heard from in a while (their process died). Goes by when the
    /// last report arrived, not by its tick: a group with a slow link is late, not gone.
    fn expire(&mut self, now: Instant) {
        let timeout = Duration::from_millis(GROUP_TIMEOUT_MS);
        let heard = &self.heard;
        self.held.retain(|g, _| {
            heard
                .get(g)
                .is_some_and(|at| now.duration_since(*at) <= timeout)
        });
        let held = &self.held;
        self.heard.retain(|g, _| held.contains_key(g));
    }

    /// An area action: pass it on, once, to every group with an entity in the circle (other
    /// than the sender). Each group delivers it to its own entities by their exact positions.
    pub fn route_area(&mut self, env: Envelope) {
        self.bus_log.push(BusMsg {
            from: env.via,
            to: self.spatial_id,
            kind: BusKind::Area,
            trace: env.trace,
        });
        let (Address::Area { x, y, r }, Kind::Action { .. }) = (env.to, &env.kind) else {
            return;
        };
        stats::AREAS.fetch_add(1, Relaxed);
        let mut groups: Vec<GroupId> = self
            .entities()
            .filter(|e| e.id != env.from.id && distance(x, y, e.x, e.y) <= r + AREA_SLACK)
            .map(|e| e.group)
            .collect();
        groups.sort_unstable();
        groups.dedup();
        for group in groups {
            let mut copy = env.clone();
            copy.via = self.spatial_id;
            self.outbox.push((group_inbox_subject(group), copy));
        }
    }

    pub fn take_outbox(&mut self) -> Vec<(String, Envelope)> {
        std::mem::take(&mut self.outbox)
    }

    /// Answer every question received since the last round, one message per asking group.
    pub fn answer(&mut self, tick: u64) -> Vec<(GroupId, SenseAnswers)> {
        let mut by_group: BTreeMap<GroupId, Vec<SenseAnswer>> = BTreeMap::new();
        self.answered.clear();
        for (group, q) in std::mem::take(&mut self.queries) {
            let seen = self.look(group, &q);
            stats::QUERIES.fetch_add(1, Relaxed);
            self.answered.push(SenseDebug {
                id: q.id,
                x: q.x,
                y: q.y,
                r: q.r,
                found: seen.len() as u32,
                spatial: vec![self.spatial_id],
            });
            by_group
                .entry(group)
                .or_default()
                .push(SenseAnswer { id: q.id, seen });
        }
        let spatial = self.spatial_id;
        by_group
            .into_iter()
            .map(|(g, answers)| {
                (
                    g,
                    SenseAnswers {
                        spatial,
                        tick,
                        answers,
                    },
                )
            })
            .collect()
    }

    /// What an entity in `group` asking `q` gets: every entity of other groups in the circle
    /// (its own group already knows its own exactly, so they'd be duplicates). If the question
    /// sets a limit (`q.nearest`), only the nearest that many, plus its target if in range. If
    /// it names components (`q.only`), only those are sent.
    fn look(&self, group: GroupId, q: &SenseQuery) -> Vec<Seen> {
        let seen = |e: &ViewEntity| {
            let parts = match &q.only {
                None => e.parts.clone(),
                Some(only) => e
                    .parts
                    .iter()
                    .filter(|(c, _)| only.contains(c))
                    .cloned()
                    .collect(),
            };
            Seen {
                id: e.id,
                x: e.x,
                y: e.y,
                group: e.group,
                player: e.player,
                kind: e.kind,
                parts,
            }
        };
        let mut found: Vec<(f32, &ViewEntity)> = self
            .held
            .iter()
            .filter(|(g, _)| **g != group)
            .flat_map(|(_, (_, list))| list.iter())
            .filter(|e| e.id != q.id)
            .filter_map(|e| {
                let d = distance(q.x, q.y, e.x, e.y);
                (d <= q.r).then_some((d, e))
            })
            .collect();
        let mut out = Vec::new();
        if let Some(i) = q
            .target
            .and_then(|t| found.iter().position(|(_, e)| e.id == t))
        {
            out.push(seen(found.swap_remove(i).1));
        }
        let n = usize::from(q.nearest);
        if n > 0 && found.len() > n {
            found.select_nth_unstable_by(n - 1, |a, b| {
                a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id))
            });
            found.truncate(n);
        }
        out.extend(found.into_iter().map(|(_, e)| seen(e)));
        out
    }

    /// Could a player at (vx, vy) see any part of our area?
    fn area_in_view(&self, vx: f32, vy: f32) -> bool {
        let Some(owners) = &self.owners else {
            return false;
        };
        owners.iter().enumerate().any(|(i, &o)| {
            let (x0, y0, x1, y1) = self.config.chunk_bounds(i);
            o == self.spatial_id && self.config.view.overlaps_box(vx, vy, x0, y0, x1, y1)
        })
    }

    /// Each player's view of our area: what they can see, always their own entity if we hold
    /// it (so the gateway learns where they went, even after a respawn), what happened there,
    /// and the questions asked there.
    pub fn build_views(&mut self, tick: u64) -> SpatialViews {
        let events = std::mem::take(&mut self.events);
        let view = self.config.view;
        let mut views = Vec::new();
        let mut watching = 0;
        for v in &self.viewers {
            let overlaps = self.area_in_view(v.x, v.y);
            let own = self.entities().any(|e| e.id == v.player);
            let involved = events.iter().any(|ev| ev.involves.contains(&v.player));
            if !overlaps && !own && !involved {
                continue;
            }
            watching += u64::from(overlaps);
            let entities: Vec<ViewEntity> = self
                .entities()
                .filter(|e| e.id == v.player || view.contains(v.x, v.y, e.x, e.y, 1.0))
                .cloned()
                .collect();
            let events: Vec<WireEvent> = events
                .iter()
                .filter(|ev| {
                    ev.involves.contains(&v.player)
                        || view.contains(v.x, v.y, ev.x, ev.y, EVENT_MARGIN)
                })
                .map(|ev| WireEvent {
                    msg: ev.msg,
                    body: ev.body.clone(),
                })
                .collect();
            let senses: Vec<SenseDebug> = self
                .answered
                .iter()
                .filter(|s| view.contains(v.x, v.y, s.x, s.y, EVENT_MARGIN))
                .cloned()
                .collect();
            views.push(CellView {
                player: v.player,
                entities,
                events,
                senses,
            });
        }
        stats::VIEWERS.store(watching, Relaxed);
        stats::VIEWS.fetch_add(views.len() as u64, Relaxed);
        SpatialViews {
            spatial: self.spatial_id,
            tick,
            views,
        }
    }

    pub fn heartbeat(&self) -> Heartbeat {
        Heartbeat {
            role: "spatial".into(),
            id: self.spatial_id,
            ..Heartbeat::default()
        }
    }

    pub fn take_debug(&mut self, tick: u64, streams: Vec<StreamMsg>) -> DebugReport {
        DebugReport {
            from: self.spatial_id,
            tick,
            bus: std::mem::take(&mut self.bus_log),
            streams,
        }
    }
}

fn distance(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
}

/// A Spatial Layer Cell process.
pub async fn run(
    nats: async_nats::Client,
    config: WorldConfig,
    spatial_id: Option<GroupId>,
) -> Result<()> {
    let spatial_id = spatial_id
        .map(|id| id | SPATIAL_ID_BIT)
        .filter(|&id| id != SPATIAL_ID_BIT)
        .unwrap_or_else(|| random_process_id(true));
    let mut world = SpatialWorld::new(spatial_id, config);

    let mut assignments = bus::subscribe::<Assignment>(&nats, ASSIGNMENTS).await?;
    let mut reports =
        bus::subscribe::<GroupReport>(&nats, &spatial_reports_subject(spatial_id)).await?;
    let mut inbox = bus::subscribe::<Envelope>(&nats, &spatial_inbox_subject(spatial_id)).await?;
    let mut viewers = bus::subscribe::<Viewers>(&nats, VIEWERS).await?;

    stats::spawn_reporter(nats.clone(), "spatial", spatial_id);
    info!(
        spatial = format!("{spatial_id:04x}"),
        "Spatial Layer Cell running"
    );
    bus::publish(&nats, HEARTBEATS.to_string(), &world.heartbeat()).await;

    // The next tick to collect reports for, and the one being collected (with when to stop
    // waiting). Aim at a tick number: messages wake this loop all the time, and "time until the
    // next tick" would jump a whole tick ahead if one arrived just after a boundary.
    let mut next = time::current_tick() + 1;
    let mut collecting: Option<(u64, Instant)> = None;
    loop {
        let now_tick = time::current_tick();
        if collecting.is_none() && now_tick >= next {
            collecting = Some((now_tick, Instant::now() + WAIT_FOR_GROUPS)); // if behind, catch up
            next = now_tick + 1;
        }
        if let Some((tick, _)) = collecting {
            if world.groups_done(tick) {
                finish_tick(&nats, &mut world, tick).await;
                collecting = None;
                continue;
            }
        }
        let wake = match collecting {
            Some((_, deadline)) => deadline,
            None => Instant::now() + time::until_tick(next),
        };
        tokio::select! {
            Some(report) = reports.recv() => world.receive_report(report),
            // Area actions go straight on, without waiting for the tick: the target groups
            // should have them before their next tick.
            Some(env) = inbox.recv() => {
                world.route_area(env);
                for (subject, env) in world.take_outbox() {
                    bus::publish(&nats, subject, &env).await;
                }
            }
            Some(v) = viewers.recv() => world.receive_viewers(v),
            Some(a) = assignments.recv() => {
                bus::set_binary(a.binary_bus);
                world.set_assignment(&a);
            }
            _ = tokio::time::sleep_until(wake) => {
                if let Some((tick, _)) = collecting.take() {
                    // Timed out: go with what we have.
                    let late: Vec<String> =
                        world.held.iter().filter(|(_, (t, _))| *t < tick).map(|(g, (t, _))| format!("{g:04x}@{t}")).collect();
                    debug!(tick, ?late, "stopped waiting for groups' reports");
                    finish_tick(&nats, &mut world, tick).await;
                }
            }
        }
    }
}

/// Split answers into messages listing at most `max_seen` entities each (whole answers only;
/// the group adds up answers for the same tick).
fn split_answers(all: SenseAnswers, max_seen: usize) -> Vec<SenseAnswers> {
    let SenseAnswers {
        spatial,
        tick,
        answers,
    } = all;
    let mut parts = vec![Vec::new()];
    let mut listed = 0;
    for a in answers {
        if listed > 0 && listed + a.seen.len() > max_seen {
            parts.push(Vec::new());
            listed = 0;
        }
        listed += a.seen.len();
        parts.last_mut().expect("never empty").push(a);
    }
    parts
        .into_iter()
        .map(|answers| SenseAnswers {
            spatial,
            tick,
            answers,
        })
        .collect()
}

/// Every group has reported (or we stopped waiting): answer questions, build views, report.
async fn finish_tick(nats: &async_nats::Client, world: &mut SpatialWorld, tick: u64) {
    let started = std::time::Instant::now();
    let id = world.spatial_id;
    world.expire(Instant::now());
    let mut streams = Vec::new();
    for (group, answers) in world.answer(tick) {
        streams.push(StreamMsg {
            from: id,
            to: group,
            kind: StreamKind::Senses,
            n: answers.answers.len() as u32,
        });
        // In parts, so no message gets near the bus's size limit however many ask.
        for part in split_answers(answers, MAX_SEEN_PER_MESSAGE) {
            bus::publish(nats, group_senses_subject(group), &part).await;
        }
    }
    if world.active {
        let views = world.build_views(tick);
        streams.push(StreamMsg {
            from: id,
            to: 0,
            kind: StreamKind::View,
            n: views.views.len() as u32,
        });
        bus::publish(nats, VIEWS.to_string(), &views).await;
    }
    bus::publish(
        nats,
        DEBUG_REPORTS.to_string(),
        &world.take_debug(tick, streams),
    )
    .await;
    if tick.is_multiple_of(time::ticks_in(500)) {
        bus::publish(nats, HEARTBEATS.to_string(), &world.heartbeat()).await;
    }
    stats::PLAYERS.store(world.entity_count() as u64, Relaxed);
    stats::record_tick(started.elapsed());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_bytes::ByteBuf;
    use worldmesh_core::config::ViewShape;
    use worldmesh_core::protocol::{Facts, Reported};

    const WEST: u32 = 0x8001;
    const EAST: u32 = 0x8002;

    fn config() -> WorldConfig {
        WorldConfig {
            width: 64,
            height: 64,
            chunk_size: 8,
            view: ViewShape::Iso {
                half_w: 32.0,
                half_h: 36.0,
            },
            input_substeps: 16,
        }
    }

    /// WEST owns chunk x < 4 (tiles x < 32), EAST the rest.
    fn west_east() -> Assignment {
        let owners = (0..64u32)
            .map(|i| if i % 8 < 4 { WEST } else { EAST })
            .collect();
        Assignment {
            version: 1,
            chunk_size: 8,
            chunks_x: 8,
            chunks_y: 8,
            owners,
            spatial: vec![WEST, EAST],
            groups: vec![1, 2],
            spatial_running: 2,
            groups_running: 2,
            group_hosts: 1,
            binary_bus: false,
            group_lag: Vec::new(),
        }
    }

    fn cell() -> SpatialWorld {
        let mut s = SpatialWorld::new(WEST, config());
        s.set_assignment(&west_east());
        s
    }

    /// Two visible components: 0 (a byte from the id) and 1 (empty).
    fn entity(id: u32, x: f32, y: f32) -> Reported {
        let parts = vec![
            (0, ByteBuf::from(vec![0x90 + (id % 10) as u8])),
            (1, ByteBuf::new()),
        ];
        Reported {
            id,
            x,
            y,
            player: id < 100,
            kind: 0,
            parts,
        }
    }

    fn report(
        group: u32,
        tick: u64,
        entities: Vec<Reported>,
        queries: Vec<SenseQuery>,
    ) -> GroupReport {
        GroupReport {
            group,
            tick,
            entities,
            queries,
            events: Vec::new(),
        }
    }

    fn area(sender: u32, x: f32, y: f32, r: f32) -> Envelope {
        Envelope {
            to: Address::Area { x, y, r },
            from: Facts {
                id: sender,
                group: 1,
                x: 10.0,
                y: 10.0,
                tick: 1000,
            },
            via: 1,
            seq: 1,
            trace: 9,
            hops: 0,
            kind: Kind::Action {
                msg: 0,
                body: vec![0xc0],
            },
        }
    }

    #[test]
    fn a_report_replaces_what_that_group_had() {
        let mut s = cell();
        s.receive_report(report(
            1,
            1000,
            vec![entity(1, 10.0, 10.0), entity(2, 12.0, 10.0)],
            vec![],
        ));
        s.receive_report(report(2, 1000, vec![entity(3, 14.0, 10.0)], vec![]));
        s.receive_report(report(1, 1001, vec![entity(2, 12.5, 10.0)], vec![])); // 1 walked out of our area
        let mut ids: Vec<u32> = s.entities().map(|e| e.id).collect();
        ids.sort();
        assert_eq!(ids, vec![2, 3]);
        assert!(
            s.entities().all(|e| e.spatial == WEST),
            "stamped with the cell that holds them"
        );
        assert_eq!(
            s.entities().find(|e| e.id == 3).unwrap().group,
            2,
            "and the group that sent them"
        );
    }

    #[test]
    fn answers_list_other_groups_in_the_circle_but_not_the_askers_own_group() {
        let mut s = cell();
        let q = SenseQuery {
            id: 100,
            x: 10.0,
            y: 10.0,
            r: 6.0,
            nearest: 0,
            target: None,
            only: None,
        };
        s.receive_report(report(
            1,
            1000,
            vec![entity(100, 10.0, 10.0), entity(1, 13.0, 10.0)],
            vec![q],
        ));
        s.receive_report(report(
            2,
            1000,
            vec![entity(2, 10.0, 15.0), entity(3, 10.0, 17.0)],
            vec![],
        ));
        let answers = s.answer(1000);
        assert_eq!(answers.len(), 1);
        let (group, a) = &answers[0];
        assert_eq!(*group, 1, "the answer goes back to the asking group");
        let seen: Vec<(u32, u32, usize)> = a.answers[0]
            .seen
            .iter()
            .map(|e| (e.id, e.group, e.parts.len()))
            .collect();
        assert_eq!(
            seen,
            vec![(2, 2, 2)],
            "1 is in the asker's own group; 3 is 7 tiles away"
        );
        assert!(s.answer(1001).is_empty(), "each question is answered once");
    }

    #[test]
    fn a_bounded_question_gets_the_nearest_ones_plus_its_target() {
        let mut s = cell();
        let mut others: Vec<Reported> = (0..20)
            .map(|i| entity(200 + i, 10.0 + 0.2 * i as f32, 11.0))
            .collect();
        others.push(entity(9, 15.0, 10.0));
        let q = SenseQuery {
            id: 100,
            x: 10.0,
            y: 10.0,
            r: 12.0,
            nearest: 3,
            target: Some(9),
            only: None,
        };
        s.receive_report(report(1, 1000, vec![entity(100, 10.0, 10.0)], vec![q]));
        s.receive_report(report(2, 1000, others, vec![]));
        let answers = s.answer(1000);
        let seen: Vec<u32> = answers[0].1.answers[0].seen.iter().map(|e| e.id).collect();
        assert_eq!(seen[0], 9, "the target comes first");
        let mut rest = seen[1..].to_vec();
        rest.sort();
        assert_eq!(rest, vec![200, 201, 202]);
    }

    #[test]
    fn a_question_can_ask_for_only_some_components() {
        let mut s = cell();
        let q = SenseQuery {
            id: 100,
            x: 10.0,
            y: 10.0,
            r: 6.0,
            nearest: 0,
            target: None,
            only: Some(vec![1]),
        };
        s.receive_report(report(1, 1000, vec![entity(100, 10.0, 10.0)], vec![q]));
        s.receive_report(report(2, 1000, vec![entity(2, 10.0, 12.0)], vec![]));
        let answers = s.answer(1000);
        let parts: Vec<u16> = answers[0].1.answers[0].seen[0]
            .parts
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(parts, vec![1]);
    }

    #[test]
    fn answers_are_split_into_bounded_parts_without_losing_any() {
        let seen = Seen {
            id: 1,
            x: 0.0,
            y: 0.0,
            group: 2,
            player: false,
            kind: 0,
            parts: vec![],
        };
        let answers: Vec<SenseAnswer> = (0..10)
            .map(|id| SenseAnswer {
                id,
                seen: vec![seen.clone(); 3],
            })
            .collect();
        let parts = split_answers(
            SenseAnswers {
                spatial: WEST,
                tick: 5,
                answers,
            },
            7,
        );
        assert_eq!(
            parts.iter().map(|p| p.answers.len()).collect::<Vec<_>>(),
            vec![2, 2, 2, 2, 2]
        );
        assert!(parts.iter().all(|p| p.tick == 5 && p.spatial == WEST));
        let empty = split_answers(
            SenseAnswers {
                spatial: WEST,
                tick: 5,
                answers: vec![],
            },
            7,
        );
        assert_eq!(empty.len(), 1, "an empty batch is still one message");
    }

    #[test]
    fn an_area_action_is_passed_on_once_to_each_group_with_someone_in_the_circle() {
        let mut s = cell();
        s.receive_report(report(
            1,
            1000,
            vec![entity(50, 10.0, 10.0), entity(1, 20.0, 20.0)],
            vec![],
        ));
        s.receive_report(report(
            2,
            1000,
            vec![entity(2, 20.5, 20.0), entity(3, 21.0, 21.0)],
            vec![],
        ));
        s.receive_report(report(3, 1000, vec![entity(4, 28.0, 28.0)], vec![]));
        s.route_area(area(50, 20.0, 20.0, 2.5));
        let out = s.take_outbox();
        let subjects: Vec<String> = out.iter().map(|(s, _)| s.clone()).collect();
        assert_eq!(
            subjects,
            vec![group_inbox_subject(1), group_inbox_subject(2)]
        );
        assert!(out
            .iter()
            .all(|(_, e)| e.via == WEST && matches!(e.to, Address::Area { .. })));
    }

    #[test]
    fn the_sender_alone_in_its_circle_gets_nothing_back() {
        let mut s = cell();
        s.receive_report(report(1, 1000, vec![entity(50, 20.0, 20.0)], vec![]));
        s.route_area(area(50, 20.0, 20.0, 2.5));
        assert!(s.take_outbox().is_empty());
    }

    #[test]
    fn a_view_has_what_is_in_view_and_always_the_viewers_own_entity() {
        let mut s = cell();
        s.receive_report(report(
            1,
            1000,
            vec![
                entity(2, 20.0, 20.0),
                entity(5, 22.0, 22.0),
                entity(6, 2.0, 60.0),
            ],
            vec![],
        ));
        s.receive_viewers(Viewers {
            tick: 1000,
            viewers: vec![
                Viewer {
                    player: 2,
                    x: 20.0,
                    y: 20.0,
                },
                Viewer {
                    player: 9,
                    x: 140.0,
                    y: 140.0,
                },
            ],
        });
        let views = s.build_views(1000);
        assert_eq!(
            views.views.len(),
            1,
            "player 9 can't see our area and isn't in it"
        );
        let mut ids: Vec<u32> = views.views[0].entities.iter().map(|e| e.id).collect();
        ids.sort();
        assert_eq!(ids, vec![2, 5], "6 is out of view");

        // After a respawn far away (still in our area), the player is in their own view.
        s.receive_report(report(1, 1001, vec![entity(2, 2.0, 2.0)], vec![]));
        s.receive_viewers(Viewers {
            tick: 1001,
            viewers: vec![Viewer {
                player: 2,
                x: 20.0,
                y: 60.0,
            }],
        });
        let views = s.build_views(1001);
        assert!(views.views[0].entities.iter().any(|e| e.id == 2));
    }

    #[test]
    fn events_go_to_the_views_that_can_see_them_and_to_whoever_is_involved() {
        let mut s = cell();
        let mut r = report(1, 1000, vec![entity(2, 20.0, 20.0)], vec![]);
        r.events.push(EventAt {
            x: 20.0,
            y: 20.0,
            msg: 0,
            body: vec![0xa1, b'a'],
            involves: vec![],
        });
        r.events.push(EventAt {
            x: 20.0,
            y: 20.0,
            msg: 0,
            body: vec![0xa1, b'b'],
            involves: vec![8],
        });
        s.receive_report(r);
        s.receive_viewers(Viewers {
            tick: 1000,
            viewers: vec![
                Viewer {
                    player: 7,
                    x: 21.0,
                    y: 21.0,
                },
                Viewer {
                    player: 8,
                    x: 5.0,
                    y: 60.0,
                },
            ],
        });
        let views = s.build_views(1000);
        let counts: Vec<(u32, usize)> = views
            .views
            .iter()
            .map(|v| (v.player, v.events.len()))
            .collect();
        assert_eq!(
            counts,
            vec![(7, 2), (8, 1)],
            "8 can't see it but was involved in the second one"
        );
    }

    #[test]
    fn a_cell_taken_out_of_use_forgets_everything() {
        let mut s = cell();
        s.receive_report(report(1, 1000, vec![entity(1, 10.0, 10.0)], vec![]));
        let mut a = west_east();
        a.spatial = vec![EAST];
        a.owners = vec![EAST; 64];
        s.set_assignment(&a);
        assert_eq!(s.entity_count(), 0);
    }

    #[test]
    fn waits_for_every_reporting_group_and_forgets_silent_ones() {
        let mut s = cell();
        assert!(s.groups_done(1000), "nobody reporting: nothing to wait for");
        s.receive_report(report(1, 1000, vec![], vec![]));
        s.receive_report(report(2, 999, vec![], vec![]));
        assert!(!s.groups_done(1000));
        s.receive_report(report(2, 1000, vec![], vec![]));
        assert!(s.groups_done(1000));
        s.expire(Instant::now());
        assert_eq!(s.held.len(), 2, "both just reported");
        s.expire(Instant::now() + Duration::from_millis(GROUP_TIMEOUT_MS + 100));
        assert!(s.held.is_empty());
    }

    #[test]
    fn a_lagging_group_is_not_waited_for_and_not_forgotten() {
        let mut s = cell();
        s.receive_report(report(1, 1000, vec![], vec![]));
        s.receive_report(report(2, 997, vec![entity(5, 10.0, 10.0)], vec![]));
        assert!(
            s.groups_done(1000),
            "waiting for a group 3 ticks behind wouldn't help anyone"
        );
        s.expire(Instant::now());
        assert_eq!(
            s.entity_count(),
            1,
            "late, not dead: its entities stay where it last put them"
        );
    }
}
