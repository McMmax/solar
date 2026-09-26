package dev.terrorify.solar;

import net.minestom.server.MinecraftServer;
import net.minestom.server.instance.Section;
import net.minestom.server.instance.block.Block;
import net.minestom.server.instance.palette.Palette;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class SolarChunkCodecTest {

    /** Round-trips a chunk with a mix of all-air, single-value, and multi-state sections. */
    @Test
    void roundTripsMixedSections() {
        MinecraftServer.init();

        final Section airSection = new Section(); // present=0 path

        final Section stoneSection = new Section();
        stoneSection.blockPalette().fill(Block.STONE.stateId());

        final Section mixedSection = new Section();
        for (int y = 0; y < 16; y++) {
            for (int z = 0; z < 16; z++) {
                for (int x = 0; x < 16; x++) {
                    final int value = ((x + y + z) % 3 == 0) ? Block.DIRT.stateId() : Block.AIR.stateId();
                    mixedSection.blockPalette().set(x, y, z, value);
                }
            }
        }

        final List<Section> original = List.of(airSection, stoneSection, mixedSection);
        final int minSection = -4;

        final byte[] encoded = SolarChunkCodec.encode(minSection, original);
        final SolarChunkCodec.Decoded decoded = SolarChunkCodec.decode(encoded);

        assertEquals(minSection, decoded.minSection());
        assertEquals(original.size(), decoded.sections().size());
        assertEquals(original.size(), decoded.lights().size());
        assertTrue(!decoded.legacyLighting());

        for (int i = 0; i < original.size(); i++) {
            final Palette expected = original.get(i).blockPalette();
            final Palette actual = decoded.sections().get(i).blockPalette();
            assertTrue(expected.compare(actual), "section " + i + " block palette mismatch");
        }
    }

    /** An all-air section must encode to the minimal present=0 byte, no palette payload. */
    @Test
    void allAirSectionIsMinimal() {
        MinecraftServer.init();

        final byte[] encoded = SolarChunkCodec.encode(0, List.of(new Section()));
        // header(4+1+1+2+1=9) + one present byte + one empty light-flags byte.
        assertEquals(11, encoded.length);

        final SolarChunkCodec.Decoded decoded = SolarChunkCodec.decode(encoded);
        assertEquals(Block.AIR.stateId(), decoded.sections().get(0).blockPalette().singleValue());
        assertEquals(0, decoded.lights().get(0).skyLight().length);
        assertEquals(0, decoded.lights().get(0).blockLight().length);
    }

    @Test
    void preservesImportedAnvilLightArrays() {
        MinecraftServer.init();
        Section section = new Section();
        byte[] sky = new byte[2048];
        byte[] block = new byte[2048];
        sky[0] = (byte) 0xF0;
        block[2047] = 0x0F;
        section.setSkyLight(sky);
        section.setBlockLight(block);

        SolarChunkCodec.Decoded decoded = SolarChunkCodec.decode(SolarChunkCodec.encode(0, List.of(section)));
        assertEquals((byte) 0xF0, decoded.lights().getFirst().skyLight()[0]);
        assertEquals((byte) 0x0F, decoded.lights().getFirst().blockLight()[2047]);
    }

    @Test
    void rejectsMalformedChunkPayloads() {
        MinecraftServer.init();

        // Null or truncated
        assertThrows(dev.terrorify.solar.exception.MalformedChunkException.class,
                () -> SolarChunkCodec.decode(null));
        assertThrows(dev.terrorify.solar.exception.MalformedChunkException.class,
                () -> SolarChunkCodec.decode(new byte[]{1, 2, 3}));

        // Bad magic
        byte[] badMagic = new byte[]{0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00};
        assertThrows(dev.terrorify.solar.exception.MalformedChunkException.class,
                () -> SolarChunkCodec.decode(badMagic));

        // Bad version
        byte[] badVersion = new byte[]{0x53, 0x6F, 0x6C, 0x72, (byte) 0x99, 0x00, 0x00, 0x00, 0x00};
        assertThrows(dev.terrorify.solar.exception.MalformedChunkException.class,
                () -> SolarChunkCodec.decode(badVersion));
    }
}
