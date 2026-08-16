# ☀️ Solar: LZ4 Streaming World Storage Engine

> **Ultra-Fast LZ4-Compressed World Storage Format, Dynamic Chunk Loader, and Block-Set Delta Logger.**  
> *Hard-proof documentation for `libs/solar` (`jvm`, `native`, `schema`).*

---

## ⚡ 1. Overview & Capabilities

* **`SolarChunkCodec.java`**: High-throughput chunk encoder/decoder utilizing LZ4 compression and 16-bit block state palettes.
* **`SolarLoader.java`**: Direct Minestom `ChunkLoader` integration loading chunks in $<100\mu	ext{s}$.
* **`SolarStore.java`**: Thread-safe storage interface querying SpacetimeDB cold storage or local region files.

---

## 📖 Canonical Specifications
* **[`.bible/03-persistence-and-storage`](file:///C:/Users/trapaholics/Documents/GitHub/.bible/03-persistence-and-storage)**
