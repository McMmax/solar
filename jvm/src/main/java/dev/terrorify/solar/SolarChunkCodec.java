package dev.terrorify.solar;

import net.minestom.server.MinecraftServer;
import net.minestom.server.instance.Chunk;
import net.minestom.server.instance.Section;
import net.minestom.server.instance.block.Block;
import net.minestom.server.instance.palette.Palette;
import net.minestom.server.network.NetworkBuffer;

import java.util.ArrayList;
import java.util.List;

import static net.minestom.server.network.NetworkBuffer.BYTE;
import static net.minestom.server.network.NetworkBuffer.INT;
import static net.minestom.server.network.NetworkBuffer.SHORT;

/**
 * Solar per-chunk byte format (see {@code libraries/solar/docs/FORMAT.md}) — the {@code data}
 * blob stored in the {@code solar_chunk} table. One blob = one chunk column.
 *
 * <p>Container: {@code magic(u32) version(u8) flags(u8) min_section(i16) section_count(u8)}
 * followed by {@code section_count} sections, bottom-to-top. Each section is a
 * {@code present(u8)} byte; if present, a block palette and (if {@code flags.has_biomes})
 * a biome palette follow.
 *
 * <p><b>Deliberate deviation from FORMAT.md's draft palette sub-format</b>: FORMAT.md's draft
 * hand-specifies {@code paletteLen/palette/wordCount/words}. This codec instead delegates the
 * palette payload directly to Minestom's own {@link Palette#BLOCK_SERIALIZER} /
 * {@link Palette#biomeSerializer(int)} (the same serializer {@code DynamicChunk} uses for the
 * network play-packet path) via {@link NetworkBuffer}. That serializer already bulk-copies the
 * palette + packed {@code long[]} with no per-block loop (FORMAT.md's actual goal) and — unlike
 * FORMAT.md's draft — already handles Minestom's "direct" storage mode (no palette array) for
 * free, since the reader recomputes the packed array length from {@code dimension}/
 * {@code bitsPerEntry} rather than needing a stored word count. Reusing it means zero risk of a
 * hand-transcribed format bug and zero new {@code Palette}/{@code PaletteImpl} accessors.
 *
 * <p>No lighting is stored (recomputed on load — see FORMAT.md). No zstd wrapping and no block
 * entities in this pass (flags bits 0 and 2 always 0) — both are documented fast-follows, not
 * correctness requirements (OPTIMIZATION.md §1).
 */
public final class SolarChunkCodec {
    private SolarChunkCodec() {}

    private static final int MAGIC = 0x536F6C72; // "Solr"
    private static final byte VERSION = 1;
    private static final int FLAG_HAS_BIOMES = 1; // bit 1

    /** Reconstructed chunk-column contents from {@link #decode}. */
    public record Decoded(int minSection, List<Section> sections) {}

    /** Encode a loaded chunk column. Convenience overload of {@link #encode(int, List)}. */
    public static byte[] encode(Chunk chunk) {
        return encode(chunk.getMinSection(), chunk.getSections());
    }

    /**
     * Encode one chunk column's sections, bottom-to-top, matching {@code DynamicChunk}'s own
     * network-write iteration order. All-air sections (single-value block palette equal to
     * {@link Block#AIR}) are written as a bare {@code present=0} byte.
     */
    public static byte[] encode(int minSection, List<Section> sections) {
        final NetworkBuffer.Type<Palette> biomeSerializer =
                Palette.biomeSerializer(MinecraftServer.getBiomeRegistry().size());

        return NetworkBuffer.makeArray(buffer -> {
            buffer.write(INT, MAGIC);
            buffer.write(BYTE, VERSION);
            buffer.write(BYTE, (byte) FLAG_HAS_BIOMES);
            buffer.write(SHORT, (short) minSection);
            buffer.write(BYTE, (byte) sections.size());

            for (Section section : sections) {
                final Palette blockPalette = section.blockPalette();
                final boolean empty = blockPalette.singleValue() == Block.AIR.stateId();
                if (empty) {
                    buffer.write(BYTE, (byte) 0);
                    continue;
                }
                buffer.write(BYTE, (byte) 1);
                buffer.write(Palette.BLOCK_SERIALIZER, blockPalette);
                buffer.write(biomeSerializer, section.biomePalette());
            }
        });
    }

    /** Decode a {@code solar_chunk.data} blob back into per-section palettes. */
    public static Decoded decode(byte[] data) {
        final NetworkBuffer buffer = NetworkBuffer.wrap(data, 0, data.length);

        final int magic = buffer.read(INT);
        if (magic != MAGIC) {
            throw new IllegalArgumentException("Solar chunk blob: bad magic 0x" + Integer.toHexString(magic));
        }
        final byte version = buffer.read(BYTE);
        if (version != VERSION) {
            throw new IllegalArgumentException("Solar chunk blob: unsupported version " + version);
        }
        final byte flags = buffer.read(BYTE);
        final boolean hasBiomes = (flags & FLAG_HAS_BIOMES) != 0;
        final int minSection = buffer.read(SHORT);
        final int sectionCount = buffer.read(BYTE) & 0xFF;

        final NetworkBuffer.Type<Palette> biomeSerializer =
                Palette.biomeSerializer(MinecraftServer.getBiomeRegistry().size());

        final List<Section> sections = new ArrayList<>(sectionCount);
        for (int i = 0; i < sectionCount; i++) {
            final byte present = buffer.read(BYTE);
            if (present == 0) {
                sections.add(new Section());
                continue;
            }
            final Palette blockPalette = buffer.read(Palette.BLOCK_SERIALIZER);
            final Palette biomePalette = hasBiomes ? buffer.read(biomeSerializer) : Palette.biomes();
            sections.add(new Section(blockPalette, biomePalette));
        }
        return new Decoded(minSection, sections);
    }
}
