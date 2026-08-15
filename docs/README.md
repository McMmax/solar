# Solar — docs index

**Solar** is a SpacetimeDB-backed Minecraft world framework — a sibling to [Polar](https://github.com/hollow-cube/polar) but bigger. Polar loads a *static, whole-world* file with no random chunk access. Solar streams a world from **SpacetimeDB**: the JVM (Minestom) fetches chunks on demand, renders them, and writes block edits back — **no world file on disk** for dynamic worlds.

> Optimization is a first-class goal of this framework, not an afterthought. Read [OPTIMIZATION.md](OPTIMIZATION.md) before implementing any phase — most design choices here exist to keep RAM, bandwidth, and the game tick cheap at multi-instance / many-player scale.

## What Solar is for
- **Lobby (today):** stays on **Polar** (static + tiny). Solar does NOT replace it.
- **Practice (future):** many ephemeral world instances cloned from a read-only template.
- **Island (future):** persistent, per-team **editable** worlds (build/upgrade), saved in SpacetimeDB.
- These gamemodes are **downstream consumers** — this framework stops at the loader + store + streaming + write-back plumbing.

## Architecture in one line
Minestom stays the authoritative game server (sim + render); **SpacetimeDB is the chunk store + delta bus + persistence**, not a simulator. (We deliberately diverge from Clockwork's `spacetimedb-minecraft`, which runs the whole sim in reducers.)

## Repo layout (`<workspace>/Solar/`)
| Dir | What | Status |
|-----|------|--------|
| `schema/` | `solar-schema` Rust crate — the `#[table]` definitions, re-exported by the cruiser STDB module | **✅ S0 done + live** |
| `native/` | `solar-bridge` — Rust FFI/transport (spacetimedb-sdk client; reuses the one connection) | ⬜ S1 |
| `jvm/` | `solar` — Gradle Java library: model + per-chunk codec + `SolarLoader` (Minestom `ChunkLoader`) + `SolarTransport` | ⬜ S2 |
| `docs/` | this folder | — |
| `FORMAT.md` (root) | canonical per-chunk byte-format spec (see [docs/FORMAT.md](FORMAT.md)) | ⬜ S2 |

## Docs
- [ARCHITECTURE.md](ARCHITECTURE.md) — the split, data model, components, and the research that justifies them.
- [ROADMAP.md](ROADMAP.md) — phases S1–S4 in detail: exact files, signatures, steps, verification.
- [OPTIMIZATION.md](OPTIMIZATION.md) — **the priority doc.** Every lever (storage, sync, write-path, read-path, RAM, CPU, multi-instance), with do/don't.
- [FORMAT.md](FORMAT.md) — the Solar per-chunk byte format.

## Current status (2026-06-05)
- **S0 (schema) — DONE & verified live.** `solar_chunk`, `solar_block_set`, `solar_world` tables + reducers (`solar_chunk_put`, `solar_block_set_apply`, `solar_block_prune`, `solar_world_upsert`, `solar_seed_from_template`) are published in the `cruiser` database and confirmed via HTTP SQL.
- **Next: S1 (native bridge).** See ROADMAP.

## Key external references
- SpacetimeDB: in-RAM store, **32 MiB** WebSocket message ceiling, subscriptions = snapshot + incremental deltas, runtime add/remove via handles. Docs: spacetimedb.com/docs (intro/key-architecture, subscriptions/semantics, tables/file-storage).
- Clockwork's `clockworklabs/spacetimedb-minecraft` — `StdbChunk` (1 row/chunk) + `StdbBlockSetUpdate` (delta log). Validates the shape; we differ on authority.
- Polar `dev.hollowcube:polar:1.15.1` — whole-world only; Solar's `FORMAT.md` is the per-chunk answer.
- Minestom `ChunkLoader` / `DynamicChunk` / `LightingChunk` / `Palette` (in the Stomzy fork).
- prismarine-chunk (`dump()/load()`), valence (`valence_anvil`/`valence_nbt`), RocksMC (per-chunk KV), region_converter (MCA/Linear, Rust) — corroborating references.
