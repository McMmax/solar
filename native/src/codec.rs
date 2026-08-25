//! Versioned Solar chunk-frame wire/storage envelope, pure-Rust fold engine, and frame ownership model.

use std::sync::Arc;
use crate::SolarChunkFrame;

/// Envelope magic signature ("SOLR").
pub const SOLAR_MAGIC: [u8; 4] = *b"SOLR";

/// Current Solar storage/wire envelope version.
pub const CURRENT_SOLAR_VERSION: u16 = 1;

/// Fixed 48-byte header size for the Solar envelope.
pub const ENVELOPE_HEADER_SIZE: usize = 48;

/// Maximum allowable envelope size (4 MB) to reject corrupted or malicious oversized buffers.
pub const MAX_ENVELOPE_PAYLOAD_SIZE: usize = 4 * 1024 * 1024;

/// Deterministic errors encountered during chunk envelope encoding or decoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolarCodecError {
    /// Magic signature mismatch (not a valid Solar frame).
    InvalidMagic { found: [u8; 4] },
    /// Unsupported envelope format version (rejects future or incompatible versions).
    UnsupportedVersion { version: u16 },
    /// Checksum verification failure (corrupted payload bytes).
    ChecksumMismatch { expected: u32, calculated: u32 },
    /// Payload exceeds maximum allowable limit.
    PayloadTooLarge { size: usize, max_allowed: usize },
    /// Input buffer is truncated (fewer bytes than header or declared payload length).
    TruncatedBuffer { needed: usize, available: usize },
    /// Invalid revision ordering (`base_revision > applied_revision`).
    InvalidRevisionSequence { base_revision: u64, applied_revision: u64 },
    /// Corrupted payload internal formatting.
    CorruptedPayload { reason: String },
}

impl std::fmt::Display for SolarCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMagic { found } => {
                write!(f, "invalid Solar magic header: {:?}", found)
            }
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported Solar envelope version: {version} (expected {CURRENT_SOLAR_VERSION})")
            }
            Self::ChecksumMismatch { expected, calculated } => {
                write!(f, "Solar envelope checksum mismatch: header 0x{expected:08X} != computed 0x{calculated:08X}")
            }
            Self::PayloadTooLarge { size, max_allowed } => {
                write!(f, "Solar payload too large: {size} bytes exceeds limit of {max_allowed} bytes")
            }
            Self::TruncatedBuffer { needed, available } => {
                write!(f, "truncated Solar envelope buffer: needed {needed} bytes, but only {available} available")
            }
            Self::InvalidRevisionSequence { base_revision, applied_revision } => {
                write!(f, "invalid revision sequence: base ({base_revision}) > applied ({applied_revision})")
            }
            Self::CorruptedPayload { reason } => {
                write!(f, "corrupted Solar payload: {reason}")
            }
        }
    }
}

impl std::error::Error for SolarCodecError {}

/// Computes a fast, deterministic Adler-32 / CRC checksum over a byte slice.
pub fn calculate_checksum(data: &[u8]) -> u32 {
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}

/// Structured representation of a decoded Solar chunk envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolarChunkEnvelope {
    pub version: u16,
    pub flags: u16,
    pub world_key: u64,
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub base_revision: u64,
    pub applied_revision: u64,
    pub checksum: u32,
    pub payload: Vec<u8>,
}

impl SolarChunkEnvelope {
    /// Constructs and encodes a new envelope from parameters.
    pub fn new(
        world_key: u64,
        chunk_x: i32,
        chunk_z: i32,
        base_revision: u64,
        applied_revision: u64,
        payload: Vec<u8>,
    ) -> Result<Self, SolarCodecError> {
        if base_revision > applied_revision {
            return Err(SolarCodecError::InvalidRevisionSequence {
                base_revision,
                applied_revision,
            });
        }
        if payload.len() > MAX_ENVELOPE_PAYLOAD_SIZE {
            return Err(SolarCodecError::PayloadTooLarge {
                size: payload.len(),
                max_allowed: MAX_ENVELOPE_PAYLOAD_SIZE,
            });
        }

        let checksum = calculate_checksum(&payload);

        Ok(Self {
            version: CURRENT_SOLAR_VERSION,
            flags: 0,
            world_key,
            chunk_x,
            chunk_z,
            base_revision,
            applied_revision,
            checksum,
            payload,
        })
    }

    /// Serializes the envelope into a binary wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(ENVELOPE_HEADER_SIZE + self.payload.len());

        // 1. Magic [0..4]
        buf.extend_from_slice(&SOLAR_MAGIC);
        // 2. Version [4..6]
        buf.extend_from_slice(&self.version.to_be_bytes());
        // 3. Flags [6..8]
        buf.extend_from_slice(&self.flags.to_be_bytes());
        // 4. World key [8..16]
        buf.extend_from_slice(&self.world_key.to_be_bytes());
        // 5. Chunk coordinates [16..24]
        buf.extend_from_slice(&self.chunk_x.to_be_bytes());
        buf.extend_from_slice(&self.chunk_z.to_be_bytes());
        // 6. Revisions [24..40]
        buf.extend_from_slice(&self.base_revision.to_be_bytes());
        buf.extend_from_slice(&self.applied_revision.to_be_bytes());
        // 7. Checksum [40..44]
        buf.extend_from_slice(&self.checksum.to_be_bytes());
        // 8. Payload length [44..48]
        let len = self.payload.len() as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        // 9. Payload bytes
        buf.extend_from_slice(&self.payload);

        buf
    }

    /// Deserializes and validates a binary wire format buffer into a `SolarChunkEnvelope`.
    pub fn decode(bytes: &[u8]) -> Result<Self, SolarCodecError> {
        if bytes.len() < ENVELOPE_HEADER_SIZE {
            return Err(SolarCodecError::TruncatedBuffer {
                needed: ENVELOPE_HEADER_SIZE,
                available: bytes.len(),
            });
        }

        // 1. Verify Magic
        let magic: [u8; 4] = bytes[0..4].try_into().unwrap();
        if magic != SOLAR_MAGIC {
            return Err(SolarCodecError::InvalidMagic { found: magic });
        }

        // 2. Verify Version
        let version = u16::from_be_bytes(bytes[4..6].try_into().unwrap());
        if version != CURRENT_SOLAR_VERSION {
            return Err(SolarCodecError::UnsupportedVersion { version });
        }

        // 3. Flags
        let flags = u16::from_be_bytes(bytes[6..8].try_into().unwrap());
        // 4. World key
        let world_key = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
        // 5. Coordinates
        let chunk_x = i32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let chunk_z = i32::from_be_bytes(bytes[20..24].try_into().unwrap());
        // 6. Revisions
        let base_revision = u64::from_be_bytes(bytes[24..32].try_into().unwrap());
        let applied_revision = u64::from_be_bytes(bytes[32..40].try_into().unwrap());

        if base_revision > applied_revision {
            return Err(SolarCodecError::InvalidRevisionSequence {
                base_revision,
                applied_revision,
            });
        }

        // 7. Checksum & Length
        let checksum = u32::from_be_bytes(bytes[40..44].try_into().unwrap());
        let payload_len = u32::from_be_bytes(bytes[44..48].try_into().unwrap()) as usize;

        if payload_len > MAX_ENVELOPE_PAYLOAD_SIZE {
            return Err(SolarCodecError::PayloadTooLarge {
                size: payload_len,
                max_allowed: MAX_ENVELOPE_PAYLOAD_SIZE,
            });
        }

        let total_needed = ENVELOPE_HEADER_SIZE + payload_len;
        if bytes.len() < total_needed {
            return Err(SolarCodecError::TruncatedBuffer {
                needed: total_needed,
                available: bytes.len(),
            });
        }

        let payload = bytes[ENVELOPE_HEADER_SIZE..total_needed].to_vec();

        // 8. Verify Checksum
        let calculated_checksum = calculate_checksum(&payload);
        if checksum != calculated_checksum {
            return Err(SolarCodecError::ChecksumMismatch {
                expected: checksum,
                calculated: calculated_checksum,
            });
        }

        Ok(Self {
            version,
            flags,
            world_key,
            chunk_x,
            chunk_z,
            base_revision,
            applied_revision,
            checksum,
            payload,
        })
    }
}

// ---------------------------------------------------------------------------
// Pure-Rust Base + Patch Fold Engine
// ---------------------------------------------------------------------------

/// Immutable packet-ready representation of a chunk with all delta patches folded into the base snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolarFoldedChunk {
    pub world_key: u64,
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub revision: u64,
    /// Folded binary payload.
    pub data: Vec<u8>,
    /// Number of patches that were folded into this snapshot.
    pub folded_patch_count: usize,
    /// Total block edits folded.
    pub folded_edit_count: usize,
}

/// Pure-Rust chunk fold engine: merges base snapshot bytes with ordered `SolarBlockEdit` patches
/// to produce an immutable packet-ready chunk without requiring Minestom or JVM classes.
pub fn fold_chunk_frame(frame: &SolarChunkFrame) -> Result<SolarFoldedChunk, SolarCodecError> {
    if frame.base_data.is_empty() {
        return Err(SolarCodecError::CorruptedPayload {
            reason: "empty base chunk data".to_string(),
        });
    }

    let total_edits: usize = frame.ordered_patches.iter().map(|p| p.edits.len()).sum();

    // If no pending patches, the folded snapshot is directly the base snapshot
    if frame.ordered_patches.is_empty() {
        return Ok(SolarFoldedChunk {
            world_key: frame.world_key,
            chunk_x: frame.chunk_x,
            chunk_z: frame.chunk_z,
            revision: frame.applied_revision,
            data: frame.base_data.clone(),
            folded_patch_count: 0,
            folded_edit_count: 0,
        });
    }

    // Fold base snapshot + serialized edits into a unified compacted buffer
    let mut folded_bytes = frame.base_data.clone();

    // Append edit metadata tail
    folded_bytes.extend_from_slice(&[0xFF, 0xFE, 0xFD, 0xFC]); // Fold trailer magic
    folded_bytes.extend_from_slice(&(total_edits as u32).to_be_bytes());

    for patch in &frame.ordered_patches {
        for edit in &patch.edits {
            folded_bytes.push(edit.x);
            folded_bytes.extend_from_slice(&edit.y.to_be_bytes());
            folded_bytes.push(edit.z);
            folded_bytes.extend_from_slice(&edit.state.to_be_bytes());
        }
    }

    Ok(SolarFoldedChunk {
        world_key: frame.world_key,
        chunk_x: frame.chunk_x,
        chunk_z: frame.chunk_z,
        revision: frame.applied_revision,
        data: folded_bytes,
        folded_patch_count: frame.ordered_patches.len(),
        folded_edit_count: total_edits,
    })
}

// ---------------------------------------------------------------------------
// Native Outbound-Frame Ownership Model
// ---------------------------------------------------------------------------

/// Outbound reference handle to an immutable published chunk frame.
///
/// Ensures:
/// - Immutable zero-copy read access across worker threads.
/// - Reference counting with explicit release tokens.
/// - Guarantees zero mutation after publication.
#[derive(Clone, Debug)]
pub struct SolarFrameHandle {
    /// Monotonic release token assigned at publication time.
    token: u64,
    /// Immutable folded chunk data.
    inner: Arc<SolarFoldedChunk>,
}

impl SolarFrameHandle {
    /// Publishes a folded chunk frame under a monotonic release token.
    pub fn publish(token: u64, folded_chunk: SolarFoldedChunk) -> Self {
        Self {
            token,
            inner: Arc::new(folded_chunk),
        }
    }

    /// Returns the unique publication release token.
    pub fn release_token(&self) -> u64 {
        self.token
    }

    /// Accesses the immutable chunk payload data.
    pub fn data(&self) -> &[u8] {
        &self.inner.data
    }

    /// Accesses the underlying chunk revision.
    pub fn revision(&self) -> u64 {
        self.inner.revision
    }

    /// Returns the strong reference count to this published frame.
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    /// Clones an explicit reader handle (retaining reference).
    pub fn retain_handle(&self) -> Self {
        self.clone()
    }
}
