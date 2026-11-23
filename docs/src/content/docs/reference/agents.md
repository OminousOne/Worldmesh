---
title: For AI agents
description: Machine-readable docs, agent skills, and the rules an agent should follow when building on Worldmesh.
---

Worldmesh is written to be easy for coding agents to build on, as well as people.

## Machine-readable docs

This site publishes the [llms.txt](https://llmstxt.org) set:

- [`/llms.txt`](/Worldmesh/llms.txt): an index of every page, with the essentials up front.
- [`/llms-full.txt`](/Worldmesh/llms-full.txt): every page in one file.
- [`/llms-small.txt`](/Worldmesh/llms-small.txt): a shorter version for small context windows.

## Agent skills

The repository's `skills/` folder has [agent skills](https://docs.claude.com/en/docs/agents-and-tools/agent-skills/overview) (a folder per skill, with a `SKILL.md`):

| Skill | Use it to |
| --- | --- |
| `worldmesh-game` | Build or change a game: the `Game` trait, `Me`, actions, queries, tests |
| `worldmesh-client` | Connect a browser client: rendering, prediction, the debug panel |
| `worldmesh-run` | Run, scale and debug a world: processes, NATS, Docker, numbers to watch |

Inside the repository they're also linked from `.claude/skills/`, so Claude Code picks them up automatically. To use them in another project, copy the folders into that project's `.claude/skills/` (or your agent's skills folder).

The repository root has an `AGENTS.md` with how to build, test and change Worldmesh itself.

## Rules that matter

The mistakes an agent is most likely to make, and what to do instead:

1. **Game code only runs inside `Game::tick`, `on_command` and `on_action`, through `Me`.** There's no global world to reach into; another entity may be on another machine.
2. **Change other entities only with actions** (`me.send`). Never keep references to other entities, or state about them outside your own entity's `Data`.
3. **Actions land on the next tick, exactly once, wherever the target is.** Don't add your own retries, acks or delays.
4. **Check actions on the receiving side with `Incoming::from` and `me.position_at(from.from.tick)`.** Never trust positions or ids a client put in a command or input.
5. **Seed randomness from (entity id, tick).** An entity can move to another group at any tick and must decide the same thing there.
6. **Keep `View` small and `query` answers pruned** (`query_with`): they dominate traffic in crowds.
7. **Match the server's movement bit for bit in the client** (`Math.fround` wherever Rust does `f32` math) if you use prediction, and check it with vectors from the Rust side.
8. **Test with `worldmesh::testing::TestWorld`**, with entities in different groups, before running servers.
