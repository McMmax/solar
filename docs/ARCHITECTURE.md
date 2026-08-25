# Solar — Architecture (Solar v1)

## The Split
```
            players (vanilla MC clients)
                     │
              ┌──────▼───────┐  authoritative sim + render
              │  Minestom    │  (Stomzy fork)
              │  (JVM)       │
              │  SolarLoader │  loadChunk()  ←─ stream chunks
              │  write hook  │  bounded edit batches  ─→
              └──────┬───────┘
                     │ Panama/FFM (jextract)
              ┌──────▼───────┐  one connection per server
              │ solar-bridge │  (Rust, spacetimedb-sdk)
              │ (native)     │  O(1) SDK cache lookup / oneway reducers
              └──────┬───────┘
                     │ WebSocket (subscriptions + reducer calls)
              ┌──────▼───────────────────────────────────┐  store + delta bus + persistence
              │ SpacetimeDB 2.8.2 (Hot module)           │  NOT a simulator
              │  solar_world                             │
              │  solar_chunk_base                        │
              │  solar_chunk_head                        │
              │  solar_chunk_patch (bounded batches)     │
              │  solar_compaction_request                │
              └──────────────────────────────────────────┘
```

**Authority:** Minestom owns gameplay, physics, and rendering. SpacetimeDB stores chunks, broadcasts bounded block deltas between instances, and persists edits atomically.

---

## Why the Experimental Model was Replaced

The experimental Solar prototype had several critical architectural flaws that degraded performance at multi-instance scale:

1. **Linear Cache Scans (Lack of Single-Column PK)**:
   SpacetimeDB client SDKs generate in-memory index accessors solely for single-column primary keys. The prototype used an auto-incrementing `id: u64` PK with a composite B-Tree index on `(world_id, chunk_x, chunk_z)`. Because client-side caches could not index on composite keys, every `loadChunk()` cache lookup required an **$O(N)$ linear iteration** across every chunk in the subscribed world.

2. **One-Row-Per-Block Table Explosion (`solar_block_set`)**:
   Storing single block edits as individual rows caused massive row count explosion in SpacetimeDB's in-memory RAM. Multi-block operations (WorldEdit, explosions, excavation) generated thousands of individual transactions and WebSocket update broadcasts.

3. **Lack of Atomic Revision Gating**:
   The prototype lacked expected-revision validation during delta writes, leading to silent race conditions and lost updates when multiple instances or asynchronous workers modified adjacent chunk areas.

4. **String World Identifier Overhead**:
   Using `String world_id` created continuous heap allocations, inflated memory footprints, and slowed key comparisons.

---

## Bounded Solar v1 Data Model (`solar-schema`)

Solar v1 replaces the experimental schema with a bounded, single-column primary key architecture:

### Key Packing Formula
Every chunk is uniquely addressed across all worlds by a packed 128-bit integer:
```text
chunk_key = (world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)
```
- **Bits 64..127**: Stable numeric `world_key: u64`.
- **Bits 32..63**: Chunk column coordinate `chunk_x: i32` cast to `u32`.
- **Bits 0..31**: Chunk column coordinate `chunk_z: i32` cast to `u32`.

This allows instant **$O(1)$ primary-key lookups** in SpacetimeDB table indices and client-side SDK memory caches.

---

### Tables

1. **`solar_world`** (Accessor: `solar_world`)
   - Primary Key: `world_key: u64`
   - Fields: `world_name`, `kind`, `owner`, `min_y`, `height`, `spawn_x..z`, `spawn_yaw`, `spawn_pitch`, `seed`, `source_template`, `created_at`, `updated_at`.
   - Index: B-Tree on `world_name`.

2. **`solar_chunk_base`** (Accessor: `solar_chunk_base`)
   - Primary Key: `chunk_key: u128`
   - Fields: `world_key`, `chunk_x`, `chunk_z`, `data: Vec<u8>` (palette-compressed chunk sections), `base_revision: u64`, `created_at`, `updated_at`.
   - Index: B-Tree on `world_key`.

3. **`solar_chunk_head`** (Accessor: `solar_chunk_head`)
   - Primary Key: `chunk_key: u128`
   - Fields: `world_key`, `chunk_x`, `chunk_z`, `applied_revision: u64`, `base_revision: u64`, `patch_count: u32`, `updated_at`, `last_actor: String`.
   - Index: B-Tree on `world_key`.

4. **`solar_chunk_patch`** (Accessor: `solar_chunk_patch`)
   - Primary Key: `id: u64` (`#[auto_inc]`)
   - Fields: `chunk_key: u128`, `world_key: u64`, `chunk_x: i32`, `chunk_z: i32`, `revision: u64`, `edits: Vec<SolarBlockEdit>`, `actor: String`, `created_at: Timestamp`.
   - Limit: Bounded batch size (`MAX_EDITS_PER_PATCH = 256`).
   - Indices: B-Tree on `(chunk_key, revision)`, B-Tree on `world_key`.

5. **`solar_compaction_request`** (Accessor: `solar_compaction_request`)
   - Primary Key: `chunk_key: u128`
   - Fields: `world_key`, `chunk_x`, `chunk_z`, `head_revision: u64`, `patch_count: u32`, `status: String`, `requested_at`, `requested_by`.
   - Threshold: Automatically triggers when `patch_count >= 64`.

---

## Reducers & State Transitions

- **`solar_world_upsert`**: Registers or updates world metadata keyed by `world_key`.
- **`solar_chunk_base_commit`**: Commits an authoritative base chunk blob and synchronizes `solar_chunk_head`.
- **`solar_chunk_apply_patch`**:
  1. Validates bounded edit limits (`1 <= edits.len() <= 256`) and coordinate ranges.
  2. Evaluates `RegionManagerAuthorizer` and `BlockBehaviorAuthorizer` authorization hooks.
  3. Validates atomic expected revision (`applied_revision == expected_revision`).
  4. Appends `SolarChunkPatch` and advances `solar_chunk_head.applied_revision`.
  5. Auto-queues `SolarCompactionRequest` if `patch_count >= 64`.
- **`solar_compaction_request_submit`**: Manually or automatically flags a chunk for compaction.
- **`solar_compaction_complete`**: Replaces `solar_chunk_base.data`, updates `base_revision`, prunes folded patches with `revision <= compacted_up_to_revision`, and recalculates remaining patch counts.
- **`solar_seed_from_template`**: Clones baseline chunk blobs from template worlds to new world instances.

---

## Authorization Integration

Solar v1 exposes explicit authorization hooks:
- **`RegionManagerAuthorizer`**: Enforces claim boundaries, safezone rules, and build permissions.
- **`BlockBehaviorAuthorizer`**: Validates block state transitions and prevents unauthorized state modifications.

Both hooks accept `SolarAuthContext { actor, is_admin, bypass_claims, client_tick }`. Admin callers and bypass flags allow privileged execution while standard edits are validated prior to any state mutation.
