# ☀️ Solar — World Persistence & Chunk Store (Solar v1)

**Solar** is a high-performance, bounded per-chunk world persistence and streaming library designed for SpacetimeDB 2.8.2 and Minestom.

---

## 🎯 Architecture & Subsystems

Solar v1 replaces the experimental per-block prototype with a bounded, single-column primary-key data model:

- **`schema/` (Rust)**: Canonical schema crate (`solar-schema`) defining the 5 core persistence tables, single-column packed `u128 chunk_key`, bounded patch batches (`MAX_EDITS_PER_PATCH = 256`), authorization hook traits, and transition invariants.
- **`native/` (Rust)**: Native bridge crate (`solar-bridge`) providing direct $O(1)$ in-memory SDK cache lookups, immutable `SolarChunkFrame` reconstruction, oneway reducer dispatch, and per-world subscriptions.
- **`jvm/` (Java)**: Minestom loader interface (`SolarLoader`) for asynchronous chunk loading and chunk streaming.

---

## 📦 Persistence Model & Tables

| Table | Primary Key | Secondary Indexes | Description |
| :--- | :--- | :--- | :--- |
| **`solar_world`** | `world_key: u64` | `world_name` (btree) | World metadata, spawn coordinates, bounds, template origin. |
| **`solar_chunk_base`** | `chunk_key: u128` | `world_key` (btree) | Palette-compressed baseline chunk snapshot (`data: Vec<u8>`). |
| **`solar_chunk_head`** | `chunk_key: u128` | `world_key` (btree) | Live monotonic revision tracking (`applied_revision`, `base_revision`, `patch_count`). |
| **`solar_chunk_patch`** | `id: u64` (auto_inc) | `chunk_key` (btree), `world_key` (btree) | Bounded edit batches (`edits: Vec<SolarBlockEdit>`, max 256 edits per row). |
| **`solar_compaction_request`** | `chunk_key: u128` | `world_key` (btree), `status` (btree) | Compaction lifecycle tracking (`pending` / `in_progress` / `failed`). |

---

## 🔑 Key Invariants & Design Principles

### 1. Single-Column Packed Primary Keys
Chunk keys are bit-packed into a single `u128`:
$$\text{chunk\_key} = (\text{world\_key} \ll 64) \mid ((\text{chunk\_x} \text{ as u32}) \ll 32) \mid (\text{chunk\_z} \text{ as u32})$$
- Eliminates multi-column composite index degradation in SpacetimeDB client SDK caches.
- Enables direct $O(1)$ point lookups in client memory.

### 2. Binary Payload Ownership & Versioning
- `solar_chunk_base.data` stores the opaque palette-compressed chunk blob.
- Binary format versioning is owned by the native/JVM engine codecs (magic bytes + version header in the blob).
- SpacetimeDB serves as an authoritative persistence and replication engine without inspecting internal byte structures.

### 3. Compaction Authority & Safe Cutoff Rule
- Amortized background compaction triggers when `patch_count >= 64`.
- **Compaction Authority**: Only authorized backend servers or compaction workers can invoke `solar_compaction_complete`.
- **Safe Cutoff Rule**: When folding patches up to `compacted_up_to_revision`, the reducer prunes **only** rows where `revision <= compacted_up_to_revision`. Newer patches committed during compaction (`revision > compacted_up_to_revision`) are strictly preserved.

### 4. Subscription & Index Strategy
- Servers selectively subscribe to individual active worlds via SQL filter queries:
  ```sql
  SELECT * FROM solar_world WHERE world_key = {world_key}
  SELECT * FROM solar_chunk_base WHERE world_key = {world_key}
  SELECT * FROM solar_chunk_head WHERE world_key = {world_key}
  SELECT * FROM solar_chunk_patch WHERE world_key = {world_key}
  SELECT * FROM solar_compaction_request WHERE world_key = {world_key}
  ```
- Uses the `world_key` B-Tree secondary index on every table to isolate network traffic.

---

## 🛠️ Implemented vs Intentionally Deferred

| Subsystem / Feature | Status | Notes |
| :--- | :--- | :--- |
| **Pinned Dependencies** | ✅ Implemented | Pinned to `spacetimedb = 2.8.2` and `spacetimedb-sdk = 2.8.2`. |
| **5 Bounded Tables & Schema** | ✅ Implemented | In `libs/solar/schema` and `local-dev/spacetime/schemas/libraries/solar/schema`. |
| **Hot Module Reducers** | ✅ Implemented | In `local-dev/spacetime/hot/src/reducers/solar.rs`. |
| **Authorization Hook Traits** | ✅ Implemented | `RegionManagerAuthorizer` & `BlockBehaviorAuthorizer` integration. |
| **Deterministic Reducer Tests** | ✅ Implemented | 16 unit tests covering revision conflicts, safe cutoff compaction, template cloning, bounds. |
| **Native Read Contract** | ✅ Implemented | `SolarChunkFrame`, `SolarChunkPatchFrame`, `reconstruct_chunk_frame`, `SolarReadError`. |
| **JVM Panama / FFI Bindings** | ⏳ Deferred | Intentionally deferred to lead integration step. |
| **Minestom Chunk Materialization** | ⏳ Deferred | Intentionally deferred; native bridge outputs immutable frames without allocating JVM chunks. |
| **Dedicated Compaction Worker** | ⏳ Deferred | Compaction reducer interface ready; worker process deferred to operational rollout. |
