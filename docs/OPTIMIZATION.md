# Solar — optimization (the priority doc)

Solar must stay cheap at **multi-instance, many-player** scale against a SpacetimeDB that **holds all data in RAM**. Every design choice below trades toward less RAM, less bandwidth, and zero tick-blocking. Read this before implementing any phase.

## The three hard constraints that drive everything
1. **SpacetimeDB is in-memory** (DB size ≈ host RAM). Every chunk you store costs server RAM forever. → store the *minimum* bytes; prune aggressively.
2. **32 MiB WebSocket message ceiling.** A single message (e.g. a subscription's initial snapshot, or one row) must fit. Per-chunk rows are tiny; **whole-world snapshots are not bounded** → region subscriptions for big worlds.
3. **The game tick is sacred (~50 ms).** Never do STDB network I/O, blocking reads, compression, or serialization on the tick thread. All reads come from the in-process SDK cache; all writes are oneway on a worker.

## Headline targets (design to these)
- Per-chunk row `data`: **single-digit KB** typical (palette-compressed, light-less, empties dropped). Hard-avoid anything approaching MBs.
- `loadChunk()`: a **cache hit** (no network) under steady state — sub-millisecond after the world subscription is applied.
- Block edit → other instances: one small `solar_block_set` row, **< a few hundred bytes**, propagated by STDB's incremental delta.
- A server's resident chunk RAM ≈ only the worlds it currently runs (per-world subscriptions).

---

## 1. Storage / row-size (biggest RAM lever)
**Do:**
- **One row per chunk column in `solar_chunk_base`**, keyed by `chunk_key: u128` (packed `(world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)`).
- **Drop fully-air chunks** — return `null` from `loadChunk` so Minestom's `Generator` makes void/flat; store nothing.
- **Omit empty sections** in the Solar format (the `[present]` flag per section). A mostly-empty island chunk should be a few hundred bytes.
- **Don't store lighting.** Recompute on load via `LightingChunk` (Stomzy already has native light compute). Saves ~4 KB/section (2048 B sky + 2048 B block). Stored light is the single largest avoidable cost.
- **zstd-compress the `data` blob** before `solar_chunk_base_commit`. Minestom palettes are already bit-packed, but zstd on the packed `long[]` + palette gives a further win for repetitive terrain. Compress/decompress on a **worker thread**, never the tick.
- **Biomes only when non-default**; **block entities only when present** (NBT), behind presence flags.
**Don't:**
- Don't store unbatched per-block rows (row count & index explosion in RAM).
- Don't store heightmaps/light you can recompute.
- Don't use linear cache scans on client reads.

## 2. Write path (edits) — bounded batches & atomic revision checks
**Do:**
- **Live edits → `solar_chunk_patch`** (bounded batches up to `MAX_EDITS_PER_PATCH = 256`), NOT whole-chunk rewrites.
- **Atomic expected-revision verification**: `solar_chunk_apply_patch` checks `applied_revision == expected_revision`, rejecting stale edits atomically.
- **Authorization gating**: Call `RegionManagerAuthorizer` and `BlockBehaviorAuthorizer` before applying state mutations.
- **Compaction lifecycle**: When `patch_count >= 64`, automatic `solar_compaction_request` triggers background folding into `solar_chunk_base` and prunes historical patches via `solar_compaction_complete`.
- **All writes oneway + off-thread** (virtual-thread executor, like `SpaceIntegration`'s existing pattern). Fire-and-forget; never await on the tick.
**Don't:**
- Don't rewrite whole base chunks on every block edit.
- Don't let patch logs grow unbounded without compaction thresholds.
- Don't block the tick waiting for write acks.

## 3. Subscriptions / sync — only what a server needs
**Do:**
- **Dynamic per-world subscriptions:** subscribe `WHERE world_key = <key>` across `solar_world`, `solar_chunk_base`, `solar_chunk_head`, `solar_chunk_patch`, `solar_compaction_request` when a `SolarLoader` attaches; **unsubscribe** when the world unloads.
- **O(1) in-memory lookups:** packed `u128 chunk_key` allows instant primary-key cache lookups without linear scanning.
- **Rely on incremental deltas:** SpacetimeDB pushes only changed rows (`solar_chunk_patch` inserts, `solar_chunk_head` updates) — subscribers converge cheaply.
**Don't:**
- Don't subscribe to all worlds' chunks (`SELECT * FROM solar_chunk_base`) — that loads every world into every server's RAM.
- Don't perform O(N) client-side cache iterations.
- Don't re-subscribe to force a refresh; the delta stream already keeps the cache live.

## 4. Read path / load — cache hits + parallelism
**Do:**
- **Read from the native SDK cache** (in-process), never a network round-trip per chunk. The world subscription pre-populates the cache so `loadChunk` is a local lookup.
- **`supportsParallelLoading()=true`** → Minestom runs `loadChunk` on virtual threads (parallel chunk-burst on join).
- **Apply pending deltas on load:** loaded chunk = `solar_chunk.data` baseline + any un-compacted `solar_block_set` for that chunk, so a fresh load is correct even before compaction.
- **Lazy deserialize:** only decode a chunk's bytes when Minestom requests it; don't eagerly decode every cached row.
- **Tune Minestom chunk-send** (`targetChunksPerTick`, `maxChunkBatchLead`) for join bursts — the demo already bumped queue limits for "the polar-world chunk-burst join spike."
- **Optional per-host byte cache** keyed `(world_id,cx,cz,rev)` to skip re-decode across instance reloads on the same machine. Secondary (cache read is already fast); add only if profiling shows decode cost.
**Don't:**
- Don't poll HTTP SQL per chunk (the lobby's one-off-read workaround) for streaming — that's per-request latency + the 2× hex inflation. SQL one-off is fine only for import/admin, not the hot path.
- Don't decode on the tick thread.

## 5. Memory (STDB RAM-bound) — keep the working set small
- Total stored bytes = Σ chunk rows + Σ live deltas. Keep both small: compress chunks, prune empties, **compact + prune deltas**.
- **Unload idle worlds** (unsubscribe) → the server cache drops those rows; STDB still holds them but the *server* RAM is freed.
- Bound island sizes (a world-border / max-chunk policy) so a runaway builder can't blow RAM.
- Consider an eviction/cold-storage policy for very stale island worlds later (export to a `.solar`/blob archive table, delete chunk rows, re-hydrate on demand) — future, not v1.

## 6. CPU / serialization
- **Bulk palette copy:** serialize via `Palette`'s raw `int[] palette` + `long[] values` (add accessors), not block-by-block iteration. Same on read (`Palette.load`). This is O(sections) not O(blocks).
- **zstd off the tick** (worker thread); pick a fast level (e.g. 3) — terrain compresses well and decode is cheap.
- **Light recompute** uses the existing native BFS (`minestom_lighting_compute_light`) — off-JVM, fast; cheaper than storing/syncing light.
- Avoid allocations in the hot read path; reuse buffers where the FFI allows.

## 7. Multi-instance / consistency
- **Read-only worlds (lobby/practice template):** many servers subscribe, **zero write contention** — the cheapest scaling mode. Practice instances all share the template's rows.
- **Editable worlds (island):** route a team's island to **one writer instance at a time** (the proxy already routes players) to avoid write conflicts. Others (if any) read. Use `rev` for staleness/optimistic checks. Last-write-wins on `solar_chunk_put` is acceptable when there's a single writer.
- **Don't** have multiple instances writing the same chunk concurrently without an owner — you'll get lost updates.

## 8. Reducer/throughput discipline
- Each reducer call = a transaction + a broadcast. **Batch** (block-set batches; coalesced chunk puts) to keep transaction rate low.
- Keep reducers **pure + tiny** (they're deterministic, no IO) — the heavy lifting (serialize, compress) is JVM-side; reducers just store bytes.
- Watch the per-second reducer rate under load; if hot, raise compaction thresholds + batch sizes.

## 9. Observability (so you can actually tune)
- Emit Solar metrics into the existing **Hyperscope** perf pipeline: chunk-load latency, cache hit rate, resident chunk count/bytes per world, delta insert rate, compaction rate, flush queue depth.
- Surface them on the CRUISER cockpit Performance page (it already renders per-server vitals) so regressions are visible.

## 10. Anti-patterns (do NOT do these)
- ❌ Whole-world blob in one row (won't sync; huge RAM).
- ❌ Whole-chunk rewrite on every block edit (churn + re-broadcast).
- ❌ Subscribe to all worlds' chunks (every server holds every island).
- ❌ Per-block rows (row/index explosion).
- ❌ Storing light/heightmaps you can recompute.
- ❌ STDB I/O, serialization, or compression on the tick thread.
- ❌ HTTP SQL per chunk on the hot path.
- ❌ Unbounded `solar_block_set` (no compaction/prune).
- ❌ Multiple concurrent writers to one editable world.

## Tuning knobs to expose (config)
- subscription mode: whole-world | windowed (+ window radius)
- compaction: trigger (delta-count threshold / interval / on-unload) + prune cadence
- flush: per-N-ticks
- compression: on/off + level
- store-light: off (default) | on
- per-host decode cache: on/off + size
- island bounds: max chunks / world-border
