# Solar — roadmap (S1–S4)

Framework only. Lobby stays on Polar. Gamemodes (practice/island) are downstream and out of scope. Apply [OPTIMIZATION.md](OPTIMIZATION.md) throughout.

Legend: ✅ done · ⬜ todo. Verify each phase before moving on.

---

## ✅ S0 — schema (DONE & live)
`Solar/schema` crate + cruiser-module wiring + publish. Tables `solar_chunk`/`solar_block_set`/`solar_world` and reducers `solar_chunk_put`/`solar_block_set_apply`/`solar_block_prune`/`solar_world_upsert`/`solar_seed_from_template` are live in the `cruiser` DB (verified via HTTP SQL). See ARCHITECTURE.md for the shapes.

**If you change the schema:** edit `Solar/schema/src/lib.rs`, then `cargo check` it, then republish the cruiser module: `cd Spacetime && spacetime publish -s local -p spacetimedb --delete-data=on-conflict -y cruiser` (additive = no wipe). Reducers live in `Spacetime/spacetimedb/src/reducers/solar.rs`.

---

## ⬜ S1 — native bridge (`solar-bridge`)
Goal: the Rust transport that the JVM calls over Panama, reusing the **single** existing SpacetimeDB connection.

**Decision — reuse, don't duplicate the connection.** The working `minestom_stdb_bridge` (`Stomzy/spacetime/native`) already owns a `DbConnection` to the cruiser DB + a generated client (`Stomzy/spacetime/stdb`). Two viable structures:
- **(A, recommended) Library + host FFI:** `Solar/native` = a Rust *library* crate `solar-bridge` with the world-chunk logic (read cache row → bytes; build reducer-call args). `minestom_stdb_bridge` depends on it and adds thin `#[ffi_export]` wrappers that pass it the live `DbConnection`. One connection, Solar owns the logic, FFI/jextract stays in the existing bridge. Coupling: `solar-bridge` depends on the generated client types (the `stdb` crate) — acceptable.
- **(B) Standalone cdylib:** `Solar/native` builds its own `.so` + own connection. Cleaner ownership but **two connections per server** — rejected per the one-connection rule unless connection-sharing is solved.

**Steps (A):**
1. **Regenerate the cruiser DB Rust client** so the native layer sees the solar tables/reducers:
   `cd Spacetime && spacetime generate --lang rust --out-dir <Stomzy>/spacetime/stdb/src/client/generated --module-path spacetimedb`
   (mirrors how the existing client was generated). Verify it includes `solar_chunk`/`solar_block_set`/`solar_world` table handles + `solar_chunk_put`/`solar_block_set_apply`/… reducer fns. **Do NOT hand-edit generated files.**
2. **`Solar/native/Cargo.toml`** — library crate `solar-bridge`; dep on the `stdb` generated client crate + `spacetimedb-sdk` (match the version the existing bridge uses). Add to a `Solar/` Cargo workspace (`schema` + `native`).
3. **`Solar/native/src/lib.rs`** — pure functions taking `&DbConnection`:
   - `get_world_chunk(conn, world_id, cx, cz) -> Option<Vec<u8>>` — `conn.db.solar_chunk().solar_chunk_loc().filter(...).next().map(|r| r.data)` (model: `hub.rs::minestom_client_get_polar_world_data`).
   - `put_world_chunk(conn, world_id, cx, cz, data)` — `conn.reducers.solar_chunk_put(...)`.
   - `apply_block_set(conn, world_id, x,y,z,state,actor)` — `conn.reducers.solar_block_set_apply(...)`.
   - `subscribe_world(conn, world_id) -> SubscriptionHandle` — `SELECT * FROM solar_chunk WHERE world_id=… ` + `… solar_block_set …`; store the handle for unsubscribe.
   - `pending_block_sets(conn, world_id, cx, cz) -> Vec<(x,y,z,state)>` — for applying un-compacted deltas on load.
4. **`Stomzy/spacetime/native/src/hub.rs`** — add `#[ffi_export]` wrappers: `solar_get_world_chunk(client, world_id, cx, cz, cb)`, `solar_world_chunk_put_oneway`, `solar_block_set_apply_oneway`, `solar_subscribe(client, world_id)`, `solar_unsubscribe(...)`. Delegate to `solar_bridge::*` with `client.inner.conn.load_full()`.
5. **Regen the C header + Java bindings** (see [native-bridge-regen]): `cargo run --bin generate_headers --features headers` → `generate_java.bat`. **Add the new `solar_*` functions to BOTH jextract include-lists** — `Stomzy/spacetime/native/generate_java.bat` AND the `RUN jextract …` block in `Stomzy/Dockerfile` (they drift; Dockerfile list is authoritative). Delete stale `gen/*.java` by hand if symbols were removed.

**Verify S1:** `cargo build --release` of the native crate + bridge; generated client compiles; `generate_headers` + `generate_java` produce `solar_*` bindings; the existing lobby still builds (don't break it).

---

## ⬜ S2 — JVM core (`Solar/jvm`, artifact `solar`)
Goal: the sibling-to-Polar Java library: model + per-chunk codec + `SolarLoader` (read path) + `SolarTransport` (FFI + subscription).

**Steps:**
1. **`Solar/jvm/build.gradle.kts`** — Gradle Java library (group e.g. `net.mcbrawls`, artifact `solar`), `compileOnly` the Minestom artifact (Stomzy publishes `net.mcbrawls:server`), publish to `mavenLocal`. Mirror `Stomzy/build.gradle.kts` conventions (blossom optional).
2. **Model:** `SolarWorld`, `SolarChunk`, `SolarSection` (per-chunk, keyed x,z).
3. **Codec:** `SolarReader`/`SolarWriter` implementing [FORMAT.md] over Minestom `Palette` — `Palette.load(int[] palette, long[] values)` on read; on write, a raw-palette getter (add `int[] palette()` + `long[] values()` accessors to Minestom `Palette`/`PaletteImpl` if not public — small fork change in Stomzy). Bulk-copy the packed `long[]`; do NOT iterate block-by-block.
4. **`SolarTransport`:** jextract bindings to the `solar_*` FFI (reuse the `SpaceIntegration` Arena/MemorySegment/callback patterns), per-world `subscribe()/unsubscribe()`, and the dirty-chunk flush queue.
5. **`SolarLoader implements net.minestom.server.instance.ChunkLoader`:**
   - `supportsParallelLoading()=true`.
   - `loadChunk(instance,cx,cz)`: `SolarTransport.getChunk(worldId,cx,cz)` (cache read) → `SolarReader` → `LightingChunk`; apply any `pending_block_sets` on top; return `null` if absent (→ Minestom `Generator`).
   - `saveChunk(chunk)`: `SolarWriter` → `solar_chunk_put` (oneway).
   - ctor `SolarLoader(worldId, boolean editable)`.

**Verify S2:** `Solar/jvm` compiles + publishes to mavenLocal; unit-test the codec round-trips a chunk (write→read equals).

---

## ⬜ S3 — write path
Goal: edits flow to STDB as live deltas + persist, with compaction.

**Steps:**
1. **Hook** `net.minestom.server.event.instance.InstanceBlockUpdateEvent` (fires after every block change incl. place/break) in `SolarTransport` for editable loaders only → `solar_block_set_apply` (oneway, on the worker executor) + mark `(worldId,cx,cz)` dirty.
2. **Debounced flush:** once per tick (or every N ticks), for each dirty chunk: serialize → `solar_chunk_put` → `solar_block_prune(world,cx,cz,maxFoldedDeltaId)`. Coalesce many same-tick edits into one `solar_chunk_put`.
3. **Read-only vs editable toggle:** read-only loaders skip the hook + never write.
4. **Cross-instance convergence:** other instances subscribed to the world receive `solar_block_set` inserts → apply to their live chunk (a `solar_block_set` on-insert callback in `solar-bridge` → JVM callback → `instance.setBlock`).

**Verify S3:** place/break a block → `solar_block_set` row appears (SQL) + `solar_chunk.rev` advances on flush; restart → edit persisted; a second instance with the same loader shows the edit live.

---

## ⬜ S4 — importer + end-to-end verify + Stomzy integration
Goal: get a world IN, wire Solar into Stomzy, prove the loop.

**Steps:**
1. **Importer** (`CRUISER/cruiserd/src/http/world.rs`, extending the existing polar→`polar_world_update` path): load a `.polar` whole-world once, iterate chunks, serialize each with the Solar format, call `solar_chunk_put` per chunk + `solar_world_upsert`. (Later: `.mca`/Linear import via `region_converter`/`valence_anvil`.)
2. **Stomzy integration:** add `solar = { ... }` to `gradle/libs.versions.toml` + `api(libs.solar)` in `build.gradle.kts`; in `Stomzy/Dockerfile` add a stage to build `Solar/native` + jextract + drop `libsolar_bridge.so` (mirror the existing `spacetime` native stage). Rebuild the `stomzy-lobby` image (`docker build -t stomzy-lobby Stomzy`).
3. **Test harness (not a gamemode):** a minimal `InstanceContainer` + `new SolarLoader("test", true)` to exercise load/edit/persist.

**Verify S4 (end-to-end):** import a small `.polar` → `world_id="test"` rows; harness streams chunks from STDB (no disk file) with correct geometry; edit → delta + persist + reload; second instance live. Confirm rows arrive over the subscription (the thing the blob couldn't do).

---

## Build/verify cheat-sheet
- STDB module: `cd Spacetime && cargo check` ; publish: `spacetime publish -s local -p spacetimedb --delete-data=on-conflict -y cruiser`.
- SQL probe: `curl -s -X POST http://127.0.0.1:3000/v1/database/cruiser/sql -H "content-type: text/plain" --data "SELECT * FROM solar_chunk"`.
- Native: `cd Stomzy/spacetime/native && cargo build --release` → `cargo run --bin generate_headers --features headers` → `generate_java.bat` (keep Dockerfile jextract list in sync).
- Client bindings: `cd Spacetime && spacetime generate --lang rust --out-dir <Stomzy>/spacetime/stdb/src/client/generated --module-path spacetimedb`.
- Lobby image: `docker build -t stomzy-lobby Stomzy` then restart the lobby (cruiser TUI `B`, or `POST /api/v1/servers/lobby/restart`).
