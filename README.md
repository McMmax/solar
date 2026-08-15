# ☀️ Solar

**Solar** is a highly optimized spatial data and geometry formatting library, featuring implementations in both **JVM (Java/Kotlin)** and **Native (Rust)**.

---

## 🎯 What does it do?
When dealing with massive game worlds, you often need to store, compress, and transmit complex geometric data—like 3D regions, chunk layouts, or spatial partitioning bounds. 

Solar provides a strictly defined data format and the tooling to read/write it. It is designed to work hand-in-hand with [Polar](../polar) to ensure that spatial data is handled as efficiently as possible before it even touches the main game loop.

## 🏗️ Architecture

- **`schema/` (Rust)**: The definitive source of truth for the Solar format. Defines exactly how bytes are packed and structured.
- **`native/` (Rust)**: High-performance native parsing and manipulation of Solar data.
- **`jvm/` (Java)**: The JVM bindings and implementations, allowing Minestom servers to seamlessly serialize and deserialize Solar formats.

## 🚀 Key Features
- **Ultra-Compact**: Designed to aggressively compress spatial data, saving network bandwidth and database storage.
- **Cross-Ecosystem**: 100% parity between the Rust native implementation and the JVM implementation.
- **Zero-Copy Architecture**: Where possible, Solar avoids duplicating memory, directly mapping byte buffers to usable objects.
