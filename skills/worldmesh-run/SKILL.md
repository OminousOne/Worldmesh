---
name: worldmesh-run
description: Run, scale and troubleshoot a Worldmesh world. Use when starting the processes (orchestrator, group, spatial, gateway) with NATS, writing Docker or compose files for them, scaling groups and Spatial Layer Cells, or reading the stats and debug panel to find a problem.
---

# Running a Worldmesh world

A world is NATS plus four roles, all from the game's one binary (`worldmesh::run(game)` reads the command line). Docs: https://ominousone.github.io/Worldmesh/guides/deployment/ and `/reference/configuration/`.

## Start everything

```sh
export NATS_URL=nats://127.0.0.1:4222 TICK_HZ=10   # same in every process
my-game orchestrator --groups 2 --spatial 2          # exactly one
my-game group                                        # any number; each runs many groups
my-game spatial                                      # any number; run spares to scale into
my-game gateway --listen 0.0.0.0:8080 --nats-monitor 127.0.0.1:8222   # add --allow-debug in development only
```

Players connect to `ws://host:8080/ws`; `GET /health` answers `ok`. `examples/medieval/docker-compose.yml` and its `Dockerfile` are a complete template (one image, role per service, `deploy.replicas` for groups and cells).

## Options

| Role | Options (env) |
| --- | --- |
| all | `--nats-url` (`NATS_URL`), `--tick-hz` (`TICK_HZ`, default 10) |
| orchestrator | `--groups` (`GROUPS`), `--spatial` (`SPATIAL`) |
| group | `--group-id` (`GROUP_ID`, default random) |
| spatial | `--spatial-id` (`SPATIAL_ID`, default random) |
| gateway | `--listen` (`LISTEN`), `--nats-monitor` (`NATS_MONITOR`), `--allow-debug` (`ALLOW_DEBUG`) |

Logging: `RUST_LOG` (default `info`; `worldmesh=debug` for per-tick detail).

## Scaling

NATS: one server, or a cluster of any size for more traffic and connections. Give every process the cluster's addresses, comma-separated, in `NATS_URL` (`nats://n1:4222,nats://n2:4222`): each connects to one at random and moves to another if it stops. Messages only travel to servers with a subscriber.

Gateways: run as many as you need behind a load balancer (no configuration; each picks its own id, player ids come from the orchestrator). A stopped gateway's players are removed after 3 s and can reconnect to any other.

Groups and cells: manual for now, from the debug panel (Groups +/-, Spatial Layer Cells +/-, or type a number), which sends `{"t":"debug","set_groups":N}` / `set_spatial` to a gateway started with `--allow-debug`. More groups re-split entities (handed off live); more cells re-split the map (nothing handed off; capped by running cell processes). Watch each group's and cell's `load_pct` (share of each tick spent working) in the metrics (M): add groups or cells past about 50%.

## Troubleshooting

| Symptom | Likely cause |
| --- | --- |
| Nobody appears; gateway logs nothing after connect | Orchestrator not running, or no group process registered yet (it waits ~3 s at start) |
| Gateway warns "no player ids from the orchestrator" and turns players away | Orchestrator not running or not reachable on NATS. It also waits 2.5 s after starting before giving out ids |
| A player vanishes and comes back after ~3 s | Their group died or stalled; the gateway re-joined them. Check that group's log and load |
| Others stutter, own entity fine | Snapshots arriving late: gateway or cell overloaded, or network. Check cell load and gateway CPU |
| Own entity corrects (snaps back) often | Inputs arriving late (see the panel's "last input arrived ... late"), or client movement code not bit-identical to the server |
| Bus traffic very high | Big spatial query results: ask only for the components you read (`Query::with::<T>()`), fewer entities (`.nearest(n)`), less often, and make fewer components visible (see the metrics' "Bus traffic by kind") |
| Actions applied late (+2 ticks or more) | A group is lagging (load near 100%, or fake lag left on: check the group legend), or sure deliveries are being sent again (the "Sure deliveries" chart; fake loss left on?) |
| Some `send` actions or inputs never land | The bus is dropping messages: a process can't keep up (load near 100%), or fake loss is on. Use `send_reliable` for what must land |
| Debug panel controls do nothing | Gateway not started with `--allow-debug` |

## Checklist for production

- [ ] No `--allow-debug` on public gateways.
- [ ] Same `TICK_HZ` and game build everywhere; host clocks synced (NTP).
- [ ] Spare cell processes running; groups scale within existing group processes.
- [ ] NATS monitoring on, gateway `--nats-monitor` set, metrics watched.
- [ ] Remember: no auth and no persistence yet.
