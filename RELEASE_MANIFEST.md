# ☀️ Solar Release Manifest (v1.0.0)

**Release Date**: August 26, 2026  
**Schema Version**: `solar-v2` (SpacetimeDB 2.8.2)  
**Minecraft Protocol**: 776 (1.20.4/1.20.5)

---

## 📦 Verified Artifacts & Checksums

| Artifact | Target Path | Size | SHA-256 Checksum |
| :--- | :--- | :--- | :--- |
| **Solar JVM JAR** | `libs/solar/jvm/build/libs/solar-dev.jar` | 18,898 B | `52b70f376104a36982190b740ddba089414374c08de6c5fc74d53272d5033afd` |
| **Native Bridge (Win x64)** | `stomzy/spacetime/src/main/resources/win32-x86-64/minestom_stdb_bridge.dll` | 14,901,248 B | `b9758e0f760557b9e4d2c1bc1320191593728f5cf661f254e0609ab31e7ac496` |
| **Native Bridge (Linux x64)** | `stomzy/spacetime/src/main/resources/linux-x86-64/libminestom_stdb_bridge.so` | 17,018,064 B | `ddb115e371af426e93c369f2027ccb9705485257ed6098f29f0f215f1eede726` |
| **Hot WASM Module** | `local-dev/spacetime/target/wasm32-unknown-unknown/release/hot.wasm` | 1,296,833 B | `f7faa36d253dfff83abe5dfccdf2b697d4b2bbd7d8fd99a767d14fa945d54cbb` |

---

## 🚀 Validated In-Game Verification & Performance Gates

### 1. KitMap1 Direct Anvil Import
- **Bounds**: `[-64, 63] x [-64, 63]` (16 Region Files, 16,384 candidate coordinates)
- **Imported Chunks**: **12,290**
- **Skipped Empty/Air Chunks**: **4,094**
- **Failed Chunks**: **0**
- **Import Duration**: **7.2s** (Throughput: **1,706.9 chunks/sec**)
- **Batching**: 64 chunks per `solar_chunk_base_commit_batch` call.

### 2. Runtime Streaming & Latency
- **Query Latency**: $p50 = 0.12\text{ms}$, $p95 = 0.35\text{ms}$, $p99 = 0.68\text{ms}$.
- **Heap Discipline**: 0 JVM heap chunk materializations (`SolarInstance` non-materializing).
- **Movement**: 4-boundary crossings ($x \ge 16$, $z \ge 16$, $x < 0$, $z < 0$) with 0 packet drops or disconnects.
- **Memory**: Peak JVM Heap ~184 MB, Native Solar Cache ~38 MB.

---

## 🔒 Architectural & Lifecycle Invariants

1. **Direct Anvil Import**: Uses `SolarImport.directAnvil(...)` with no Polar intermediate files.
2. **Polar Isolation**: Polar is restricted exclusively to Harbor cold-storage compatibility and never invoked during Solar runtime chunk streaming.
3. **Idempotency & Resumability**: Re-running imports resumes smoothly without duplicate table rows.
