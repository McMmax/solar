package dev.terrorify.solar;

import dev.terrorify.solar.exception.MalformedChunkException;
import net.minestom.server.MinecraftServer;
import net.minestom.server.instance.Chunk;
import net.minestom.server.instance.Section;
import net.minestom.server.instance.block.Block;
import net.minestom.server.instance.palette.Palette;
import net.minestom.server.network.NetworkBuffer;
import org.jetbrains.annotations.NotNull;

import java.util.ArrayList;
import java.util.List;

import static net.minestom.server.network.NetworkBuffer.BYTE;
import static net.minestom.server.network.NetworkBuffer.INT;
import static net.minestom.server.network.NetworkBuffer.SHORT;

/**
 * High-performance binary codec for Solar chunk columns (see {@code libraries/solar/docs/FORMAT.md}).
 *
 * <p>Encodes and decodes the canonical {@code data} blob stored in SpacetimeDB's {@code solar_chunk_base}
 * table. One blob represents a full vertical chunk column.
 *
 * <h2>Container Format</h2>
 * <pre>{@code
 * magic(u32: 'Solr') | version(u8: 1) | flags(u8) | min_section(i16) | section_count(u8)
 * followed by section_count sections:
 *   present(u8: 0=empty, 1=populated)
 *   [if present==1]: block_palette (Minestom NetworkBuffer) | biome_palette (if flags.has_biomes)
 * }</pre>
 */
public final class SolarChunkCodec {
    private SolarChunkCodec() {}

    private static final int MAGIC = 0x536F6C72; // "Solr"
    private static final byte VERSION = 1;
    private static final int FLAG_HAS_BIOMES = 2; // bit 1

    /** Reconstructed chunk-column contents from {@link #decode}. */
    public record Decoded(int minSection, @NotNull List<Section> sections) {}

    /** Encode a loaded chunk column. Convenience overload of {@link #encode(int, List)}. */
    public static byte[] encode(@NotNull Chunk chunk) {
        return encode(chunk.getMinSection(), chunk.getSections());
    }

    /**
     * Encode one chunk column's sections, bottom-to-top, matching {@code DynamicChunk}'s own
     * network-write iteration order. All-air sections (single-value block palette equal to
     * {@link Block#AIR}) are written as a bare {@code present=0} byte.
     *
     * @param minSection inclusive minimum section Y index (e.g. -4 for 1.18+ overworld)
     * @param sections   list of chunk sections bottom-to-top
     * @return canonical Solar encoded byte array
     */
    public static byte[] encode(int minSection, @NotNull List<Section> sections) {
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

    /**
     * Decode a canonical Solar chunk blob back into per-section block and biome palettes.
     *
     * @param data Solar chunk byte blob
     * @return decoded chunk structure containing minSection and section list
     * @throws MalformedChunkException if magic, version, or payload structure is invalid
     */
    public static Decoded decode(byte @NotNull [] data) {
        if (data == null || data.length < 9) {
            throw new MalformedChunkException("Solar chunk blob is truncated or empty");
        }
        final NetworkBuffer buffer = NetworkBuffer.wrap(data, 0, data.length);

        final int magic = buffer.read(INT);
        if (magic != MAGIC) {
            throw new MalformedChunkException("Solar chunk blob: bad magic 0x" + Integer.toHexString(magic));
        }
        final byte version = buffer.read(BYTE);
        if (version != VERSION) {
            throw new MalformedChunkException("Solar chunk blob: unsupported version " + version);
        }
        final byte flags = buffer.read(BYTE);
        final boolean hasBiomes = (flags & FLAG_HAS_BIOMES) != 0 || (flags & 1) != 0;
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
