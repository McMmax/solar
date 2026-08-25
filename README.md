# ☀️ Solar

**Solar** is a highly optimized per-chunk world persistence and streaming library for SpacetimeDB and Minestom, featuring implementations in both **JVM (Java/Kotlin)** and **Native (Rust)**.

---

## 🎯 Architecture (Solar v1)

Solar v1 replaces the experimental per-block prototype with a bounded, SpacetimeDB 2.8.2-friendly data model:

- **`schema/` (Rust)**: Canonical schema crate (`solar-schema`) defining the 5 core persistence tables, single-column packed `u128 chunk_key`, bounded patch batches (`MAX_EDITS_PER_PATCH = 256`), and authorization hook traits.
- **`native/` (Rust)**: Native bridge (`solar-bridge`) providing fast $O(1)$ in-memory SDK cache lookups, oneway reducer dispatch, and per-world subscriptions.
- **`jvm/` (Java)**: Minestom `SolarLoader` implementing asynchronous chunk streaming and local delta handling.

## 🚀 Key Features

- **Single-Column Packed Keys**: `chunk_key = (world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)` guarantees instant $O(1)$ primary key lookups without linear cache scans.
- **Bounded Edit Batches**: Mutations are grouped into bounded `solar_chunk_patch` batches, preventing table row explosion in SpacetimeDB RAM.
- **Atomic Revision Fencing**: Reducers enforce optimistic revision checks (`applied_revision == expected_revision`) before committing patches.
- **Explicit Authorization Hooks**: Gated through `RegionManagerAuthorizer` and `BlockBehaviorAuthorizer` before state mutation.
- **Amortized Compaction**: Background compactor folds delta patches into `solar_chunk_base` and prunes historical logs when threshold is reached.
