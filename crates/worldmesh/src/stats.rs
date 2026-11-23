//! Per-process metrics for the debug graphs: CPU, memory, bus and WebSocket traffic, and
//! (for groups and Spatial Layer Cells) how long each tick takes. Every process publishes its own once a
//! second; the gateway adds NATS's numbers and forwards everything to players.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use crate::protocol::{ProcessStats, Rate};
use crate::subjects::STATS;
use crate::time;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::bus;

/// Running totals of messages and bytes, reset at every report.
pub struct Counter {
    msgs: AtomicU64,
    bytes: AtomicU64,
}

impl Counter {
    pub const fn new() -> Self {
        Self { msgs: AtomicU64::new(0), bytes: AtomicU64::new(0) }
    }

    pub fn add(&self, bytes: usize) {
        self.msgs.fetch_add(1, Relaxed);
        self.bytes.fetch_add(bytes as u64, Relaxed);
    }

    fn take_rate(&self, secs: f32) -> Rate {
        Rate {
            msgs: self.msgs.swap(0, Relaxed) as f32 / secs,
            bytes: self.bytes.swap(0, Relaxed) as f32 / secs,
        }
    }
}

/// Bus messages received and published by this process.
pub static BUS_IN: Counter = Counter::new();
pub static BUS_OUT: Counter = Counter::new();
/// WebSocket messages from and to players (gateway only).
pub static WS_IN: Counter = Counter::new();
pub static WS_OUT: Counter = Counter::new();
/// Players connected (gateway), entities simulated (group) or held (Spatial Layer Cell).
pub static PLAYERS: AtomicU64 = AtomicU64::new(0);
/// Spatial Layer Cell: sense questions answered. (Groups count per group, in group.rs: one
/// process can run many.)
pub static QUERIES: AtomicU64 = AtomicU64::new(0);
/// Spatial Layer Cell: views built, area effects passed on, and players watching right now.
pub static VIEWS: AtomicU64 = AtomicU64::new(0);
pub static AREAS: AtomicU64 = AtomicU64::new(0);
pub static VIEWERS: AtomicU64 = AtomicU64::new(0);

static TICK_US_SUM: AtomicU64 = AtomicU64::new(0);
static TICK_US_MAX: AtomicU64 = AtomicU64::new(0);
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

/// A group or Spatial Layer Cell spent `took` on one tick.
pub fn record_tick(took: Duration) {
    let us = took.as_micros() as u64;
    TICK_US_SUM.fetch_add(us, Relaxed);
    TICK_US_MAX.fetch_max(us, Relaxed);
    TICK_COUNT.fetch_add(1, Relaxed);
}

/// Publish this process's stats once a second, forever.
pub fn spawn_reporter(nats: async_nats::Client, role: &'static str, id: u32) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut last = Instant::now();
        let mut last_cpu = cpu_time();
        loop {
            interval.tick().await;
            let now = Instant::now();
            let secs = now.duration_since(last).as_secs_f32().max(0.001);
            last = now;

            let cpu = cpu_time();
            let cpu_pct = match (last_cpu, cpu) {
                // A thread that exited takes its time with it; never report negative use.
                (Some(before), Some(after)) => Some(after.saturating_sub(before).as_secs_f32() / secs * 100.0),
                _ => None,
            };
            last_cpu = cpu;

            let ticks = TICK_COUNT.swap(0, Relaxed);
            let (sum, max) = (TICK_US_SUM.swap(0, Relaxed), TICK_US_MAX.swap(0, Relaxed));
            let stats = ProcessStats {
                role: role.to_string(),
                id,
                cpu_pct,
                mem_mb: rss_mb(),
                bus_in: BUS_IN.take_rate(secs),
                bus_out: BUS_OUT.take_rate(secs),
                ws_in: WS_IN.take_rate(secs),
                ws_out: WS_OUT.take_rate(secs),
                players: PLAYERS.load(Relaxed) as u32,
                tick_ms_avg: (ticks > 0).then(|| sum as f32 / ticks as f32 / 1000.0),
                tick_ms_max: (ticks > 0).then(|| max as f32 / 1000.0),
                load_pct: (ticks > 0).then(|| load_pct(sum, ticks)),
                queries: QUERIES.swap(0, Relaxed) as f32 / secs,
                views: VIEWS.swap(0, Relaxed) as f32 / secs,
                areas: AREAS.swap(0, Relaxed) as f32 / secs,
                viewers: VIEWERS.load(Relaxed) as u32,
                bus_kinds: bus::MsgKind::ALL
                    .iter()
                    .zip(&bus::OUT_BY_KIND)
                    .map(|(kind, counter)| (kind.name().to_string(), counter.take_rate(secs).bytes))
                    .collect(),
                ..ProcessStats::default()
            };
            bus::publish(&nats, STATS.to_string(), &stats).await;
        }
    });
}

/// Load: the average share of a tick spent working, in %, from `ticks` ticks totalling `sum_us`.
pub fn load_pct(sum_us: u64, ticks: u64) -> f32 {
    sum_us as f32 / ticks as f32 / 1000.0 / time::tick_ms() as f32 * 100.0
}

/// CPU time this process has used. Linux only (the server runs in Docker).
fn cpu_time() -> Option<Duration> {
    threads_cpu_time().or_else(process_cpu_time)
}

/// Sum of every thread's on-CPU time, in ns (precise enough for tiny loads).
fn threads_cpu_time() -> Option<Duration> {
    let mut ns = 0u64;
    for task in std::fs::read_dir("/proc/self/task").ok()? {
        let Ok(text) = std::fs::read_to_string(task.ok()?.path().join("schedstat")) else { continue };
        ns += text.split_whitespace().next()?.parse::<u64>().ok()?;
    }
    (ns > 0).then(|| Duration::from_nanos(ns))
}

/// User + system time from /proc/self/stat (10 ms resolution).
fn process_cpu_time() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Skip "pid (command name) "; the name may contain spaces. Field 3 (state) is then
    // index 0, so utime (field 14) and stime (field 15) are at 11 and 12.
    let fields: Vec<&str> = stat.get(stat.rfind(')')? + 2..)?.split_whitespace().collect();
    let clock_ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    Some(Duration::from_millis(clock_ticks * 10)) // 100 clock ticks per second on Linux
}

/// Resident memory in MB. Linux only.
fn rss_mb() -> Option<f32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: f32 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// NATS's own numbers, from its monitoring endpoint (`host:port`, e.g. `nats:8222`).
pub struct NatsMonitor {
    addr: String,
    /// Time and totals (in msgs, out msgs, in bytes, out bytes) at the last sample.
    last: Option<(Instant, [f64; 4])>,
}

impl NatsMonitor {
    pub fn new(addr: String) -> Self {
        Self { addr, last: None }
    }

    /// NATS as one more process in the graphs: CPU, memory, and messages in (published
    /// to it) and out (delivered to subscribers) per second.
    pub async fn sample(&mut self) -> Option<ProcessStats> {
        let body = http_get(&self.addr, "/varz").await?;
        let v: serde_json::Value = serde_json::from_slice(&body).ok()?;
        let num = |key: &str| v.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let totals = [num("in_msgs"), num("out_msgs"), num("in_bytes"), num("out_bytes")];
        let now = Instant::now();
        let (bus_in, bus_out) = match self.last.replace((now, totals)) {
            Some((then, prev)) => {
                let secs = now.duration_since(then).as_secs_f64().max(0.001);
                let per_sec = |i: usize| ((totals[i] - prev[i]).max(0.0) / secs) as f32;
                (Rate { msgs: per_sec(0), bytes: per_sec(2) }, Rate { msgs: per_sec(1), bytes: per_sec(3) })
            }
            None => (Rate::default(), Rate::default()),
        };
        Some(ProcessStats {
            role: "nats".to_string(),
            cpu_pct: Some(num("cpu") as f32),
            mem_mb: Some((num("mem") / 1024.0 / 1024.0) as f32),
            bus_in,
            bus_out,
            ..ProcessStats::default()
        })
    }
}

/// Minimal HTTP GET (the monitoring endpoint is plain HTTP on the private network).
async fn http_get(addr: &str, path: &str) -> Option<Vec<u8>> {
    let request = async {
        let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
        let req = format!("GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n");
        stream.write_all(req.as_bytes()).await.ok()?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.ok()?;
        let body = buf.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
        Some(buf[body..].to_vec())
    };
    tokio::time::timeout(Duration::from_millis(500), request).await.ok().flatten()
}
