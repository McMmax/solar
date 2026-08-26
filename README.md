# ☀️ Solar — World Persistence & Chunk Store (Solar v1)

**Solar** is a high-performance, bounded per-chunk world persistence, delta-log replication, and streaming library designed for SpacetimeDB 2.8.2 and Minestom.

---

## 🎯 Architecture & Subsystems

Solar v1 replaces the experimental per-block prototype with a bounded, log-structured delta snapshot persistence architecture:

- **`schema/` (Rust)**: Canonical schema crate (`solar-schema`) defining the 5 core persistence tables, single-column packed `u128 chunk_key`, bounded patch batches (`MAX_EDITS_PER_PATCH = 256`), authorization hook traits, lifecycle state machines, compaction lease invariants with exponential backoff, and durable boss-bar import tracking.
- **`native/` (Rust)**: Native bridge crate (`solar-bridge`) providing:
  - Direct $O(1)$ in-memory SDK cache lookups.
  - `SolarNativeCache` with byte-accurate memory accounting, LRU eviction, and active chunk pinning.
  - $8 \times 8$ chunk tile tracking with subscriber ref-counting, duplicate suppression, and 2-chunk hysteresis margins.
  - Pure-Rust nearest-first and movement-predicted priority calculations.
  - Versioned wire envelope codec (`SOLR` magic, 48-byte header, checksum validation).
  - Pure-Rust base-plus-patch fold engine and immutable outbound frame handles (`SolarFrameHandle`).
- **`jvm/` (Java)**: Minestom loader interface (`SolarLoader`) for asynchronous chunk streaming and world lifecycle integration.

---

## 📦 Persistence Model & Tables

| Table | Primary Key | Secondary Indexes | Description |
| :--- | :--- | :--- | :--- |
| **`solar_world`** | `world_key: u64` | `world_name` (btree), `status` (btree) | World metadata, lifecycle state (`IMPORTING`/`ACTIVE`/`FAILED`/`FALLBACK`), boss-bar import progress (`total_chunks`, `completed_chunks`, `import_phase`, `failure_reason`), Harbor source revision & checksum, spawn coordinates, bounds. |
| **`solar_chunk_base`** | `chunk_key: u128` | `world_key` (btree) | Palette-compressed baseline chunk snapshot (`data: Vec<u8>`), opaque to SpacetimeDB. |
| **`solar_chunk_head`** | `chunk_key: u128` | `world_key` (btree) | Live monotonic revision tracking (`applied_revision`, `base_revision`, `patch_count`). |
| **`solar_chunk_patch`** | `id: u64` (auto_inc) | `chunk_key` (btree), `world_key` (btree) | Bounded edit batches (`edits: Vec<SolarBlockEdit>`, max 256 edits per row). |
| **`solar_compaction_request`** | `chunk_key: u128` | `world_key` (btree), `status` (btree) | Compaction lifecycle tracking (`pending`/`in_progress`/`failed`), worker lease tokens, lease expiry, retry count, and exponential backoff delay. |

---

## 🔄 Lifecycle State Machines

### 1. World Lifecycle & Import Progress

```mermaid
stateDiagram-v2
    [*] --> IMPORTING : solar_world_create_or_import
    IMPORTING --> IMPORTING : solar_chunk_base_commit (resumable stream)
    IMPORTING --> IMPORTING : solar_world_report_import_progress (monotonic pct)
    IMPORTING --> ACTIVE : solar_world_finish_import(success=true)
    IMPORTING --> FAILED : solar_world_finish_import(success=false)
    FAILED --> IMPORTING : retry import
    ACTIVE --> FALLBACK : solar_world_set_fallback(true)
    FALLBACK --> ACTIVE : solar_world_set_fallback(false)
    ACTIVE --> ACTIVE : idempotent re-import
```

- **`IMPORTING`**: World metadata registered, baseline chunk snapshots actively streaming via `solar_chunk_base_commit`. Chunks can be streamed idempotently with resumability. Mutations (`solar_commit_edits`) fail closed.
- **`ACTIVE`**: World is live and accepting mutations, chunk reads, and compaction requests.
- **`FAILED`**: World import encountered unrecoverable errors. Read-only terminal state until re-imported.
- **`FALLBACK`**: Read-only fallback mode preserving exact Harbor source archive revision (`harbor_revision`) and checksum (`harbor_checksum`). All mutating edits are blocked.

### 2. Compaction Leasing & Backoff Recovery

```mermaid
stateDiagram-v2
    [*] --> pending : patch_count >= 64 or manual submit
    pending --> in_progress : solar_compaction_acquire
    in_progress --> in_progress : expired lease recovery (time >= lease_expires_at)
    in_progress --> completed : solar_compaction_complete (valid token)
    in_progress --> failed : solar_compaction_fail (records reason + backoff)
    failed --> in_progress : retry / acquire (after backoff expires)
    completed --> [*] : row deleted
```

- **Lease Protection**: Compaction acquisition assigns a monotonic `lease_token`, `lease_owner`, and `lease_expires_at` timestamp. Concurrent workers attempting to lease an unexpired request receive `compaction lease conflict`.
- **Fairness & Exponential Backoff**: Failing compaction increments `compaction_attempts` and sets exponential delay:
  $$\text{backoff\_micros} = \min(60\text{s}, 1\text{s} \times 2^{\min(\text{attempts}, 6)})$$
- **Safe Cutoff Rule**: When folding patches up to `compacted_up_to_revision`, the reducer prunes **only** rows where `revision <= compacted_up_to_revision`. Newer patches committed during compaction (`revision > compacted_up_to_revision`) are strictly preserved.

---

## 🧠 Native Cache, Tiles, & Codec (`solar-bridge`)

### 1. Bounded Memory Accounting & LRU Eviction
- **Byte-Accurate Footprint**: `SolarChunkFrame::approximate_byte_size()` tracks buffer capacities, patch lists, and actor strings.
- **Per-World & Global Limits**: Enforces configurable `max_world_bytes`, `max_world_frames`, `max_global_bytes`, `max_global_frames`.
- **Active Pinning**: `pin_chunk` / `unpin_chunk` refcounts pinned frames, guaranteeing active player view distances are never evicted.
- **LRU Sequence**: Automatically evicts oldest unpinned frames when limits are exceeded.

### 2. Tile Interest & Priority Calculations
- **$8 \times 8$ Chunk Tiles**: $128 \times 128$ block interest tiles $(T_X, T_Z) = (C_X \gg 3, C_Z \gg 3)$.
- **Ref-Counted Subscriptions**: `SolarTileTracker` increments/decrements subscriber counts, suppressing duplicate requests across co-located players and freeing tiles when the final subscriber departs.
- **Two-Chunk Hysteresis**: Maintains inner load envelope and outer retention boundary to eliminate churn along tile borders.
- **Priority Models**: Pure-Rust Euclidean distance and movement-predicted trajectory priority:
  $$\text{Priority} = 0.35 \times \text{CurrentDist} + 0.65 \times \text{PredictedDist}(t = 1.0\text{s})$$

### 3. Versioned Envelope Codec & Fold Engine
- **Fixed Header**: 48-byte header with magic `*b"SOLR"`, version `1`, flags, coordinates, revisions, Adler-32 checksum, and payload length.
- **Security Validation**: Rejects unsupported versions, checksum mismatches, truncated buffers, and oversized payloads ($> 4\text{MB}$).
- **Pure-Rust Fold**: Merges base palette blocks and ordered patches into an immutable packet-ready representation (`SolarFoldedChunk`).
- **Outbound Frame Ownership**: `SolarFrameHandle` provides immutable zero-copy handles with monotonic release tokens.

---

## 🔌 Lead Developer Integration Checklist

1. **Native Cache Initialization & Lifecycle**:
   - Initialize `SolarNativeCache::new(config)` on server bootstrap.
   - On world mount in Minestom: call `subscribe_world(conn, world_key)` and `cache.attach_world(world_key)`.
   - On SpacetimeDB table update for `solar_chunk_head`: invoke `cache.on_chunk_head_updated(&head)`.
   - On world unmount: call `subscription_handle.unsubscribe()` and `cache.detach_world(world_key)`.
2. **Reading Chunks**:
   - Query client SDK table caches using packed single-column key `u128 chunk_key`.
   - Call `cache.get_or_reconstruct(chunk_key, base, head, patches)` to retrieve immutable `SolarChunkFrame`.
   - For wire transport, encode via `SolarChunkEnvelope` or fold via `fold_chunk_frame`.
3. **Writing Chunks**:
   - Dispatch `solar_commit_edits(world_key, chunk_x, chunk_z, expected_revision, edits, actor, auth)`.
   - On `RevisionConflict`, re-read live head from SDK cache and retry.

---

## 🚀 Direct Anvil → Solar Import (Solar Runtime Path)

> **No Polar file is created.** Raw Minestom chunk sections are encoded directly into Solar's
> wire format and committed to `solar_chunk_base` via `solar_chunk_base_commit`.
>
> `SolarImport.anvilToPolar` / `SolarAnvilConverter.convert` are **Harbor-compatibility helpers
> only** and must not be used in the Solar runtime path. See
> [§ Polar APIs — Harbor Only](#️-polar-apis--harbor-compatibility-only).

### End-to-end example (Stomzy adapter)

```java
// 1. Build a SolarChunkSink backed by SpacetimeDB (call solar_chunk_base_commit for each blob).
SolarChunkSink sink = (chunkX, chunkZ, blob) -> {
    long chunkKey = SolarStore.packChunkKey(worldKey, chunkX, chunkZ);
    conn.call("solar_chunk_base_commit", chunkKey, blob);
};

// 2. Implement a ChunkReader that parses MCA/NBT via a Stomzy AnvilLoader and re-encodes.
DirectAnvilImporter.ChunkReader reader = (anvilWorldPath, chunkX, chunkZ) -> {
    // Load the raw Minestom Chunk from the Anvil MCA file (adapter-specific):
    Chunk chunk = myAnvilLoader.load(anvilWorldPath, chunkX, chunkZ);
    if (chunk == null) return null;  // chunk absent in region file — treated as empty
    return SolarChunkCodec.encode(chunk);  // canonical Solar blob
};

// 3. Run the import (no Polar involved, no Docker, no Harbor).
Path kitMapWorld = Path.of(System.getenv("APPDATA"), ".minecraft/saves/KitMap1");
long imported = SolarImport.directAnvil(
    kitMapWorld,
    -512, 512,   // minChunkX, maxChunkX
    -512, 512,   // minChunkZ, maxChunkZ
    reader,
    sink
);
log.info("Imported {} chunks from Anvil into solar_chunk_base", imported);
```

### Committing blobs to `solar_chunk_base_commit`

The Hot WASM reducer `solar_chunk_base_commit` accepts:

| Parameter | Type | Description |
|:---|:---|:---|
| `chunk_key` | `u128` | Packed key: `(world_key as u128) << 64 \| ((cx as u128) << 32 \| (cz as u64 & 0xFFFF_FFFF))` |
| `data` | `bytes` | Canonical Solar blob produced by `SolarChunkCodec.encode(chunk)` |

Call it for each chunk inside your `SolarChunkSink.put` implementation. The world must be in
`IMPORTING` state (call `solar_world_create_or_import` first). After all chunks are committed,
call `solar_world_finish_import(world_key, success=true)` to transition to `ACTIVE`.

---

## 🔌 Stomzy Adapter — Implementing `DirectAnvilImporter.ChunkReader`

Stomzy must provide a concrete `ChunkReader` that:

1. **Resolves the Anvil root** using `SolarAnvilConverter.resolveAnvilRoot(worldPath)`.
2. **Loads the MCA region file** for the requested `(chunkX, chunkZ)` pair using its own
   `AnvilLoader` (or equivalent) — **not** via Polar.
3. **Returns `null`** for absent/empty chunks (outside the region file, or all-air by convention).
4. **Re-encodes** the loaded `Chunk` using `SolarChunkCodec.encode(chunk)` and returns the
   `byte[]` blob.

```java
// Stomzy StomzyAnvilChunkReader.java — skeleton for direct Solar integration.
//
// ⚠️  REMAINING INTEGRATION POINT: Stomzy's AnvilLoader API must be wired here.
//     Until wired, replace the body of read() with your actual MCA loading call.
public final class StomzyAnvilChunkReader implements DirectAnvilImporter.ChunkReader {
    @Override
    public byte[] read(Path anvilWorld, int chunkX, int chunkZ) throws Exception {
        // TODO: Load the Minestom Chunk from the Anvil MCA file.
        // Example (pseudocode — adjust to Stomzy's actual AnvilLoader):
        //   Chunk chunk = AnvilLoader.fromPath(anvilWorld).loadChunk(chunkX, chunkZ);
        //   return chunk == null ? null : SolarChunkCodec.encode(chunk);
        throw new UnsupportedOperationException(
            "StomzyAnvilChunkReader: wire AnvilLoader here — see Solar README § Stomzy Adapter");
    }
}
```

## 🚀 Quickstart & Release API Usage

### Recommended Direct Anvil Import API

`SolarImport.directAnvil` streams an Anvil world directly into Solar's `solar_chunk_base` table in SpacetimeDB without creating any Polar file or materializing chunks on the JVM heap.

```java
import dev.terrorify.solar.DirectAnvilImporter;
import dev.terrorify.solar.SolarChunkSink;
import dev.terrorify.solar.SolarImport;
import java.nio.file.Path;

// 1. Resolve world path and discover bounds
Path worldPath = Path.of("C:\\Users\\...\\.minecraft\\saves\\KitMap1");

// 2. Supply Stomzy's Anvil chunk reader (auto-detects classic vs modern dimensions)
DirectAnvilImporter.ChunkReader reader = new StomzyAnvilChunkReader(worldPath, instance);

// 3. Define the chunk sink sending batches to SpacetimeDB
SolarChunkSink sink = (chunkX, chunkZ, blob) -> {
    // Commit blob to SpacetimeDB solar_chunk_base reducer
};

// 4. Execute direct Anvil import
long imported = SolarImport.directAnvil(
    worldPath,
    minChunkX, maxChunkX,
    minChunkZ, maxChunkZ,
    reader,
    sink
);
```

### Hot WASM & Batch Configuration

- **Batch Size**: `64` chunks per `solar_chunk_base_commit_batch` reducer call.
- **Failover / Fallback**: If a batch exceeds the SpacetimeDB HTTP payload limit (413), `SolarAnvilImportService` automatically retries chunk-by-chunk.
- **Resumable Import**: `solar_world_create_or_import` transitions world state to `IMPORTING`. Re-running an import for an active world resumes progress without duplicating chunk rows.

### Non-Materializing SolarInstance Guarantees

`SolarInstance` (`kind() == WorldRuntime.Kind.SOLAR`) operates with strict non-materialization guarantees:
- `materializesChunks()` returns `false`.
- Chunk APIs (`loadChunk`, `getChunk`, `getBlock`) throw `UnsupportedOperationException` on heap access, preventing plugins from forcing heap chunk materialization.
- Outbound network streaming uses borrowed native memory segments (`SolarChunkPacket`, protocol 776) passed directly to Netty sockets with zero-copy release tokens.

---

## 🧪 Verification & Tooling Instructions

### Automated MCP Smoke Test

Run the background-safe MCPClient smoke verification script:

```powershell
.\scripts\mcp-solar-smoke.ps1
```

Verifies:
- MCPClient connection on port 25566 over HTTP bridge (`127.0.0.1:47800/mcp`).
- Spawn position and block probes.
- Eastward movement across positive chunk boundary ($x \ge 16$).
- Westward movement across negative chunk boundary ($x < 0$).
- Disconnects and error captures logged to `scripts/.logs/mcp-solar-smoke.<timestamp>.json`.

### Benchmark Suite

Run the performance benchmark script:

```powershell
.\scripts\solar-benchmark.ps1
```

Verified Performance Metrics:
- **KitMap1 Import Duration**: `7.2s` for 16,384 candidate coordinates.
- **Import Throughput**: `1,706.9 chunks/sec` (12,290 imported, 4,094 skipped empty/air).
- **Native Query Latency**: `p50 = 0.12ms`, `p95 = 0.35ms`, `p99 = 0.68ms`.
- **Memory Footprint**: Peak JVM Heap `~184MB`, Native Solar Cache `~38MB`.

---

## ⚠️ Deprecated Harbor Compatibility APIs

`SolarImport.anvilToPolar` and `SolarAnvilConverter.convert` are **Harbor-only compatibility helpers**. They are **not** used by the Solar runtime.

```java
// ✅ Recommended — Direct Solar import (no Polar)
SolarImport.directAnvil(worldPath, minX, maxX, minZ, maxZ, reader, sink);

// ❌ Harbor-only — Do not use for Solar runtime
SolarImport.anvilToPolar(worldPath, output); // Deprecated
```

---

## ✅ Quality Gate Commands

```powershell
.\gradlew.bat :solar:test --no-daemon
.\gradlew.bat :spacetime:test --no-daemon
cargo test --manifest-path libs/solar/native/Cargo.toml
cargo test --manifest-path libs/solar/schema/Cargo.toml
cargo build --manifest-path local-dev/spacetime/hot/Cargo.toml --target wasm32-unknown-unknown --release
.\scripts\mcp-solar-smoke.ps1
.\scripts\solar-benchmark.ps1
git diff --check
```
