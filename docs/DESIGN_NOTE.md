# Solar v1 Architecture & Design Note

## 1. System Overview

Solar v1 is the persistence engine for voxel worlds in Stomzy, built on SpacetimeDB 2.8.2. It replaces earlier un-batched per-block prototypes with a bounded, log-structured delta snapshot persistence architecture.

---

## 2. Core Architectural Invariants

### 2.1 Single-Column Packed Keys
SpacetimeDB client SDK caches generate lookup maps only on single primary key columns. To avoid $O(N)$ linear cache scans across all chunks:
- Each chunk column is assigned a 128-bit integer primary key:
  $$\text{chunk\_key} = (\text{world\_key} \ll 64) \mid ((\text{chunk\_x} \text{ as u32}) \ll 32) \mid (\text{chunk\_z} \text{ as u32})$$
- World-to-local coordinate extraction uses euclidean remainder and bitwise shifting:
  - Local $X = \text{world\_x.rem\_euclid}(16)$
  - Local $Z = \text{world\_z.rem\_euclid}(16)$
  - Chunk $X = \text{world\_x} \gg 4$
  - Chunk $Z = \text{world\_z} \gg 4$

### 2.2 Bounded Mutation Batches
Individual block edits are grouped into `solar_chunk_patch` rows with a hard ceiling of `MAX_EDITS_PER_PATCH = 256`.
- Limits row amplification in SpacetimeDB server memory.
- Reduces WebSocket transaction broadcast overhead during mass block edits.
- Protects client and server frame rates during explosions and procedural generation.

### 2.3 Atomic Revision Fencing (Optimistic Concurrency)
- Every patch commit must supply `expected_revision`.
- If `expected_revision != solar_chunk_head.applied_revision`, the reducer aborts immediately with `revision conflict`.
- In concurrent multi-actor races on the same chunk column, exactly one writer succeeds and advances the revision; all others receive deterministic conflict errors.

### 2.4 Compaction Authority & Safe Cutoff Rule
- When `solar_chunk_head.patch_count >= COMPACTION_THRESHOLD_PATCHES (64)`, a compaction request is queued.
- An authorized compactor reads the base snapshot, folds historical patches up to `compacted_up_to_revision`, and calls `solar_compaction_complete`.
- **Safe Cutoff Rule**: The reducer replaces `solar_chunk_base.data`, updates `solar_chunk_head.base_revision = compacted_up_to_revision`, and prunes only patches with `revision <= compacted_up_to_revision`. Any patches committed concurrently after `compacted_up_to_revision` remain untouched in `solar_chunk_patch`.

### 2.5 Subscription & Network Isolation
- All tables contain a secondary B-Tree index on `world_key`.
- Servers subscribe only to active instances via `SELECT * FROM table WHERE world_key = ...`.
- Offloads inactive instance chunks from memory and network traffic.

---

## 3. Implementation Status

### Implemented
- [x] Schema definition and validation helpers (`libs/solar/schema`, `local-dev/spacetime/schemas/libraries/solar/schema`).
- [x] Server reducers and authorization hooks (`local-dev/spacetime/hot/src/reducers/solar.rs`).
- [x] 16 deterministic schema and reducer unit tests (`cargo test` passing).
- [x] Native read contract and frame reconstruction (`libs/solar/native`, 6 unit tests passing).
- [x] SpacetimeDB 2.8.2 dependency pinning across all workspaces.

### Intentionally Deferred
- [ ] JVM Panama / FFI bindings (deferred to lead integration).
- [ ] Minestom `Chunk` and `Section` object materialization (deferred to lead integration).
- [ ] Standalone compaction daemon process (deferred to deployment phase).
