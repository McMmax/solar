# Solar Scripts & Reproducible Tooling Command Reference

This directory contains executable PowerShell scripts for building, running, importing, and verifying the Solar environment.

---

## 🛠️ Scripts & Usage

### 1. `run.ps1`
Launches SpacetimeDB (port 3000) and Stomzy (`demo.bat`) in the background.

```powershell
.\scripts\run.ps1 [-NoBuild] [-SeedWorld]
```

- `-NoBuild`: Skip rebuilding Stomzy via Gradle before launching.
- `-SeedWorld`: Calls `seed-world.ps1` to create an empty active world row for protocol debugging.

### 2. `import-anvil.ps1`
Imports a Minecraft Anvil world save directly into SpacetimeDB's `solar_chunk_base` table.

```powershell
.\scripts\import-anvil.ps1 [-AnvilWorld <path>] [-NoBuild] [-SkipEnvCheck]
```

- `-AnvilWorld`: Path to the Minecraft save directory (defaults to `KitMap1`).
- Auto-detects classic (`world/region`) vs modern (`world/dimensions/minecraft/overworld/region`) dimension layouts.

### 3. `mcp-solar-smoke.ps1`
Automated, background-safe smoke test driving MCPClient.

```powershell
.\scripts\mcp-solar-smoke.ps1 [-Port 25566]
```

- Connects to Stomzy, probes spawn blocks, executes movement across positive and negative chunk boundaries, and saves results to `scripts/.logs/mcp-solar-smoke.<timestamp>.json`.

### 4. `solar-benchmark.ps1`
Automated performance benchmark suite script.

```powershell
.\scripts\solar-benchmark.ps1
```

- Measures import metrics, native query latency ($p50/p95/p99$), JVM/native memory usage, and multi-boundary movement.
- Outputs machine-readable results to `scripts/.logs/solar-benchmark.<timestamp>.json`.

### 5. `stop.ps1`
Cleanly stops only background processes started by Solar scripts (read from `scripts/.logs/pids.json` or matched by data directory). Never touches user Minecraft worlds.

```powershell
.\scripts\stop.ps1
```
