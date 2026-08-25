# Solar per-chunk byte format (`data` in `solar_chunk_base`)

The Solar sibling to Polar's `FORMAT.md`, but **per-chunk** (independently (de)serializable — the thing Polar can't do). One `solar_chunk_base.data` blob = one baseline chunk column snapshot. Produced/consumed JVM-side from Minestom `Palette`; opaque to the SpacetimeDB module.

Status: **v1 specification**

## Goals
- Independently (de)serializable per chunk (keyed by `chunk_key: u128`).
- Tiny: palette-compressed, **no lighting** (recomputed on load), empty sections dropped, optional zstd wrapper.
- Version-stable (a `version` byte; reader switches on it).
- Cheap to encode/decode: bulk-copy Minestom `Palette` arrays, no per-block iteration.

## Container
All integers **little-endian**. `varint` = LEB128 unsigned unless noted.

```
magic        : u32 = 0x536F6C72   ("Solr")
version      : u8  = 1
flags        : u8  (bit0 = zstd-compressed payload; bit1 = has biomes; bit2 = has block entities)
min_section  : i16 (lowest section index, e.g. -4 for min_y=-64)
section_count: u8  (number of section slots described below)
payload      : [if flags.zstd: zstd frame wrapping the SECTIONS+EXTRAS block; else raw]
```
The compressed payload (recommended) contains:
```
SECTIONS : section_count × Section
EXTRAS   : [if flags.has_block_entities] BlockEntities
           (user-data reserved; see below)
```

### Section
Sections are listed bottom→top (`min_section` upward). Empty/air-only sections are encoded with `present=0` (1 byte) and nothing else.
```
present : u8 (0 = empty/all-air → skip; 1 = present)
if present:
  blockPalette : Palette         (block state ids)
  if flags.has_biomes:
    biomePalette : Palette        (biome ids, 4×4×4 resolution)
```

### Palette (mirrors Minestom `Palette.load(int[] palette, long[] values)`)
A direct, bulk-copyable encoding of Minestom's paletted container:
```
bitsPerEntry : u8                  (0 = single-value section)
if bitsPerEntry == 0:
  value      : varint              (the one state/biome id filling the section)
else:
  paletteLen : varint
  palette    : paletteLen × varint (distinct state/biome ids)
  wordCount  : varint              (length of the packed long[])
  words      : wordCount × i64     (Minestom's packed values; copied verbatim)
```
Write = read `Palette`'s internal `int[] palette` + `long[] values` (add `int[] palette()` / `long[] values()` / `int bitsPerEntry()` accessors to Minestom `Palette`/`PaletteImpl` in the Stomzy fork). Read = `Palette.load(palette, words)` (or `fill(value)` for the single-value case). **No block-by-block loop.**

### BlockEntities (only if flags.has_block_entities)
```
count : varint
entry × count:
  x : u8  (0..15, section-relative not needed; use chunk-relative)
  y : i16 (world Y)
  z : u8
  nbt : adventure-nbt CompoundBinaryTag (length-prefixed)
```

### User data (reserved)
Polar gives each chunk an arbitrary "user data" blob. Reserve a trailing optional section (a `flags` bit + `varint len + bytes`) for Solar to carry gamemode metadata per chunk later (e.g. island ownership, region tags) without a format break.

## Notes
- **Lighting is intentionally absent.** On load, build a `LightingChunk` and let Minestom (native BFS) recompute. If a future profile shows recompute is too costly for a workload, add a `flags` bit + appended per-section sky/block nibble arrays (2×2048 B) — but default off (see OPTIMIZATION.md §1).
- **zstd** wraps the post-header payload (header stays readable to peek `version`/bounds without decompressing). Compress/decompress off the tick thread.
- **Heightmaps** are not stored (Minestom derives them). 
- Keep the header tiny and fixed so a reader can validate `magic`/`version`/bounds cheaply before decompressing.

## Versioning
- Bump `version` on any incompatible change; the reader keeps old-version paths.
- `rev` (on the `solar_chunk` row, not in this blob) tracks mutation revision for staleness/concurrency — separate concern.

## Reference encoders to mirror
- Minestom `Palette` (the source of truth for block storage in this stack).
- prismarine-chunk `dump()/load()` (paletted network format) — conceptual parallel.
- Polar's per-chunk layout (sections + palette + user data) — Solar is the per-row-addressable version of the same idea.
