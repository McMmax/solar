# Solar — architecture

## The split
```
            players (vanilla MC clients)
                     │
              ┌──────▼───────┐  authoritative sim + render
              │  Minestom    │  (Stomzy fork)
              │  (JVM)       │
              │  SolarLoader │  loadChunk()  ←─ stream chunks
              │  write hook  │  block edits  ─→
              └──────┬───────┘
                     │ Panama/FFM (jextract)
              ┌──────▼───────┐  one connection per server
              │ solar-bridge │  (Rust, spacetimedb-sdk)
              │ (native)     │  reads SDK cache / calls reducers
              └──────┬───────┘
                     │ WebSocket (subscriptions + reducer calls)
              ┌──────▼───────┐  store + delta bus + persistence
              │ SpacetimeDB  │  (cruiser module; solar-schema tables)
              │  solar_chunk │  NOT a simulator
              │  solar_block_set
              │  solar_world │
              └──────────────┘
```

**Authority:** Minestom owns gameplay/physics/rendering. SpacetimeDB only stores chunks, broadcasts block deltas between instances, and persists edits. This is the deliberate opposite of Clockwork's `spacetimedb-minecraft` (whole sim in reducers) — ours is lighter and keeps tick latency off the network.

## Data model (`solar-schema`, live in the `cruiser` DB)
- **`solar_chunk`** — canonical per-chunk column state. One row per `(world_id, chunk_x, chunk_z)` (uniqueness enforced by the upsert reducer; btree index `solar_chunk_loc`). Fields: `id u64 auto_inc` PK, `world_id`, `chunk_x`, `chunk_z`, `data: Vec<u8>` (Solar per-chunk bytes — see FORMAT.md), `rev: u64` (bumped each write), `updated_at`.
- **`solar_block_set`** — live block-edit delta log (Clockwork `StdbBlockSetUpdate` analogue). Fields: `id u64 auto_inc` PK, `world_id`, `chunk_x`, `chunk_z` (derived `x>>4`/`z>>4`), `x`,`y`,`z`, `state: i32` (Minestom state id), `actor`, `at`; btree `solar_block_loc`.
- **`solar_world`** — per-world metadata. PK `world_id`; `kind` (lobby|practice_template|island), `owner` (team), `min_y`, `height`, `spawn_*`, `seed`, `source_template`, `created_at`.

### Reducers (the framework API; in the cruiser module `reducers/solar.rs`)
- `solar_chunk_put(world_id, chunk_x, chunk_z, data)` — upsert one chunk, bump `rev`.
- `solar_block_set_apply(world_id, x, y, z, state, actor)` — append one delta (chunk coords derived).
- `solar_block_prune(world_id, chunk_x, chunk_z, up_to_id)` — drop deltas folded into the chunk.
- `solar_world_upsert(world_id, kind, owner, min_y, height, spawn_x..pitch, seed, source_template)`.
- `solar_seed_from_template(src_world_id, dst_world_id, owner)` — copy a template's chunk rows + meta into a new world.

## Why these shapes (research-backed)
1. **Per-chunk rows, not a blob.** Polar can't slice; a monolithic ~196 KB blob *would not sync over a SpacetimeDB subscription* (empirically) though it returned via one-off HTTP SQL. Per-chunk rows are tiny, sync cleanly, and allow on-demand/region loading. Clockwork's `StdbChunk` is the same shape.
2. **Delta log + compaction, not whole-chunk rewrites per edit.** Editing a chunk by rewriting its whole row on every block change is wasteful and noisy across instances. `solar_block_set` carries single-block edits as small rows (cheap to broadcast); they fold into `solar_chunk` periodically (`solar_chunk_put` + `solar_block_prune`).
3. **Minestom-authoritative.** STDB reducers at ~20 Hz/player with a network round-trip per move would be heavy; keeping sim in Minestom and using STDB as store/bus is the right cost profile (and is what the user wants: "JVM retrieves chunks and renders them; JVM sends block updates").
4. **One native connection per server.** SpacetimeDB's Rust SDK keeps a local cache fed by subscriptions; `solar-bridge` reads that cache in-process (zero round-trip per chunk) and writes via oneway reducer calls — reusing the existing `minestom_stdb_bridge` connection rather than opening a second one.

## Components
- **`solar-bridge` (native, S1):** FFI `solar_subscribe(world_id)`, `solar_get_world_chunk(world_id,cx,cz,cb)` (cache read → bytes), `solar_world_chunk_put_oneway`, `solar_block_set_apply_oneway`. Modeled on `Stomzy/spacetime/native/src/hub.rs` (`minestom_client_get_polar_world_data`, the `*_oneway` reducers, the `calculate_knockback` cache-read). Needs the cruiser DB's generated Rust client (regenerated to include the solar tables/reducers).
- **`SolarLoader` (JVM, S2):** implements Minestom `ChunkLoader`. `loadChunk` → `SolarTransport.getChunk` → `SolarReader` → `LightingChunk` (null → generator). `saveChunk` → `SolarWriter` → `solar_chunk_put`. `supportsParallelLoading()=true`. Read-only vs editable flag.
- **`SolarTransport` (JVM, S2):** Panama bindings to `solar-bridge` + per-world subscription lifecycle + the `InstanceBlockUpdateEvent` write hook.
- **Per-chunk codec (JVM, S2):** `SolarReader`/`SolarWriter` over Minestom `Palette` (`load(int[],long[])` + a raw-palette getter). See FORMAT.md.

## Integration points (existing repos)
- **cruiser module** (`Spacetime/spacetimedb`): already re-exports `solar-schema` + mounts reducers (S0 done).
- **Stomzy native bridge** (`Stomzy/spacetime/native` + `stdb` generated client): S1 adds Solar FFI + regenerates the client bindings; **both jextract include-lists** (`generate_java.bat` AND the `Stomzy/Dockerfile` RUN block) must stay in sync.
- **Stomzy build** (`gradle/libs.versions.toml`, `build.gradle.kts`): add `solar` like `polar`; Dockerfile builds `Solar/native` + jextract + drops the `.so`.
- **Lobby** (`Stomzy/demo/.../PlayerInit.java`): untouched — keeps Polar.

## Constraints to respect
- SpacetimeDB holds **all data in RAM** (≈ host RAM) → bound world sizes, compress chunk blobs, prune empties.
- **32 MiB** WebSocket message ceiling → per-chunk rows are fine, but a whole-world initial subscription snapshot is the sum of its chunk rows; very large worlds need **region/windowed subscriptions** (OPTIMIZATION.md).
- Reducers are deterministic/transactional (no IO, `ctx.rng` only) — keep them pure; all game logic stays JVM-side.
- Never block the game tick on STDB I/O — all reads come from the local cache, all writes are oneway on a worker thread.
