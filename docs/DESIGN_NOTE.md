# Solar v1 Architecture & Design Note

## 1. System Overview

Solar v1 is the authoritative world persistence, delta replication, and caching engine in Stomzy, built on SpacetimeDB 2.8.2. It replaces earlier un-batched per-block prototypes with a bounded, log-structured delta snapshot persistence architecture.

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

### 2.4 World Lifecycle State Transitions & Boss-Bar Tracking
The world store enforces explicit lifecycle transitions:
- `IMPORTING`: Baseline chunk snapshot streaming via `solar_chunk_base_commit`. Resumable chunk streaming and monotonic progress percentage tracking (`progress_pct = (completed * 100) / total`). Mutations fail closed.
- `ACTIVE`: Normal live world operations. Reads, mutations, and compaction permitted.
- `FAILED`: Unrecoverable import/verification failure. Read-only terminal state until re-imported.
- `FALLBACK`: Read-only fallback to Harbor world storage. Solar edits rejected (`WorldInFallback`). Pinned `harbor_revision` and `harbor_checksum` are strictly preserved.

### 2.5 Compaction Authority, Safe Cutoff Rule, & Exponential Backoff
- When `solar_chunk_head.patch_count >= COMPACTION_THRESHOLD_PATCHES (64)`, a compaction request is queued with `status = "pending"`.
- A compactor worker acquires a lease via `solar_compaction_acquire`, setting `status = "in_progress"`, `lease_token`, and `lease_expires_at`.
- Expired leases ($T \ge \text{lease\_expires\_at}$) can be recovered automatically by another worker.
- If compaction fails (`solar_compaction_fail`), attempts are incremented and exponential backoff is enforced before re-acquisition:
  $$\text{backoff\_micros} = \min(60\text{s}, 1\text{s} \times 2^{\min(\text{attempts}, 6)})$$
- **Safe Cutoff Rule**: The reducer replaces `solar_chunk_base.data`, updates `solar_chunk_head.base_revision = compacted_up_to_revision`, and prunes only patches with `revision <= compacted_up_to_revision`. Any patches committed concurrently after `compacted_up_to_revision` remain untouched in `solar_chunk_patch`.

### 2.6 Native Cache Memory Controls & LRU Eviction (`SolarNativeCache`)
- Per-world and global memory budgets (`max_world_bytes`, `max_world_frames`, `max_global_bytes`, `max_global_frames`).
- Accurate byte accounting tracking data buffers, edit arrays, and actor metadata.
- Pinned frames (`pin_chunk` / `unpin_chunk`) are protected from eviction.
- LRU eviction of unpinned frames when memory or frame limits are reached.

### 2.7 8x8 Tile Interest Tracking & Hysteresis Primitives
- Chunks mapped to $8 \times 8$ tiles: $(T_X, T_Z) = (C_X \gg 3, C_Z \gg 3)$.
- Reference-counted subscriptions prevent duplicate network and memory loading across co-located players.
- Two-chunk hysteresis margin prevents rapid thrashing along interest zone boundaries.
- Pure-Rust Euclidean and velocity-extrapolated movement-predicted priority calculations.

### 2.8 Versioned Wire Envelope Codec & Fold Engine
- Fixed 48-byte header: `SOLR` magic, version `1`, flags, world key, chunk coords, revisions, Adler-32 checksum, payload length.
- Pure-Rust base-plus-patch folding (`fold_chunk_frame`) producing immutable `SolarFoldedChunk`.
- `SolarFrameHandle` with monotonic release tokens ensuring publication immutability.

---

## 3. Reducer Interface Contract

| Reducer | Inputs | Guarantees / Behavior |
| :--- | :--- | :--- |
| `solar_world_create_or_import` | `world_key`, `world_name`, `kind`, `owner`, `total_chunks`, bounds, spawn, template, `harbor_rev`, `harbor_csum` | Sets `status = IMPORTING`. Initializes boss-bar tracking. Idempotent if matching active world. |
| `solar_world_report_import_progress` | `world_key`, `completed_chunks`, `import_phase` | Monotonically advances completed chunks count and phase. |
| `solar_world_finish_import` | `world_key`, `success: bool`, `error_message` | Transitions `IMPORTING -> ACTIVE` or `IMPORTING -> FAILED`. |
| `solar_world_set_fallback` | `world_key`, `enable_fallback: bool` | Transitions `ACTIVE <-> FALLBACK`. Locks/unlocks mutations. |
| `solar_chunk_base_commit` | `world_key`, `chunk_x`, `chunk_z`, `data: Vec<u8>`, `base_revision` | Commits baseline chunk blob. Allowed in `IMPORTING` or `ACTIVE`. Idempotent on resume. |
| `solar_commit_edits` | `world_key`, `chunk_x`, `chunk_z`, `expected_revision`, `edits: Vec<SolarBlockEdit>`, `actor`, `auth` | Enforces RegionManager -> BlockBehavior -> ContainerAccess auth, checks `expected_revision`, writes patch, advances head, auto-queues compaction. |
| `solar_compaction_acquire` | `world_key`, `chunk_x`, `chunk_z`, `lease_owner`, `lease_duration_micros` | Acquires exclusive lease; validates backoff eligibility; assigns monotonic `lease_token`. |
| `solar_compaction_complete` | `world_key`, `chunk_x`, `chunk_z`, `lease_token`, `compacted_data`, `compacted_up_to_revision` | Verifies lease token, replaces base blob, prunes folded patches $\le$ cutoff rev, updates head, deletes request. |
| `solar_compaction_fail` | `world_key`, `chunk_x`, `chunk_z`, `lease_token`, `reason` | Verifies lease token, sets `status = failed`, records failure reason, computes exponential backoff delay. |
| `solar_seed_from_template` | `src_world_key`, `dst_world_key`, `dst_world_name`, `owner` | Clones all base chunks with distinct packed keys; initializes dst world as `ACTIVE`. |

---

## 4. Implementation & Verification Summary

- **`libs/solar/schema`**: 28 unit tests passing (`cargo test`).
- **`libs/solar/native`**: 18 unit tests passing (`cargo test`).
- **`local-dev/spacetime/schemas/libraries/solar/schema`**: 28 unit tests passing (`cargo test`).
- **`local-dev/spacetime/hot`**: Verified compilation (`cargo check` 0 errors).
