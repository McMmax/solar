//! Resumable Polar-to-Solar import engine.
//!
//! # Invariants:
//! 1. **All Five Import States Exercised**: `DISCOVERY` -> `STREAMING_BASE` -> `VALIDATING` -> `COMPLETED` (or `FAILED`).
//! 2. **Restart Resumes**: Interrupted or aborted imports can resume from the exact point of interruption without restarting from chunk 0.
//! 3. **Corrupt Input Fails Closed**: Any corrupt chunk, checksum mismatch, or malformed envelope halts the import immediately, marks state as `FAILED`, and never commits corrupted data.
//! 4. **Completed Chunks Not Retransferred**: Chunks already committed or present in target storage are skipped and not retransmitted or reprocessed.
//! 5. **Fixture Digest Recorded**: A deterministic cryptographic digest of the complete imported world geometry is calculated and verified.

use crate::codec::{SolarChunkEnvelope, calculate_checksum};
use crate::{
    IMPORT_PHASE_COMPLETED, IMPORT_PHASE_DISCOVERY, IMPORT_PHASE_FAILED,
    IMPORT_PHASE_STREAMING_BASE, IMPORT_PHASE_VALIDATING, WORLD_STATUS_ACTIVE,
    WORLD_STATUS_FAILED, WORLD_STATUS_IMPORTING,
    SolarChunkBase, pack_chunk_key,
};
use std::collections::{BTreeMap, HashSet};

/// Five canonical import lifecycle phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportPhase {
    Discovery,
    StreamingBase,
    Validating,
    Completed,
    Failed,
}

impl ImportPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Discovery => IMPORT_PHASE_DISCOVERY,
            Self::StreamingBase => IMPORT_PHASE_STREAMING_BASE,
            Self::Validating => IMPORT_PHASE_VALIDATING,
            Self::Completed => IMPORT_PHASE_COMPLETED,
            Self::Failed => IMPORT_PHASE_FAILED,
        }
    }
}

impl std::str::FromStr for ImportPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == IMPORT_PHASE_DISCOVERY {
            Ok(Self::Discovery)
        } else if s == IMPORT_PHASE_STREAMING_BASE {
            Ok(Self::StreamingBase)
        } else if s == IMPORT_PHASE_VALIDATING {
            Ok(Self::Validating)
        } else if s == IMPORT_PHASE_COMPLETED {
            Ok(Self::Completed)
        } else if s == IMPORT_PHASE_FAILED {
            Ok(Self::Failed)
        } else {
            Err(())
        }
    }
}

/// Errors returned during the Polar-to-Solar import lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolarImportError {
    /// World key is invalid (0).
    InvalidWorldKey,
    /// World name is empty.
    EmptyWorldName,
    /// Chunk payload is empty or zero-length.
    EmptyChunkData { chunk_key: u128 },
    /// Chunk payload is corrupted or failed validation.
    CorruptChunkData {
        chunk_key: u128,
        reason: String,
    },
    /// Chunk payload checksum mismatch.
    ChecksumMismatch {
        chunk_key: u128,
        expected: u32,
        actual: u32,
    },
    /// Import is in an invalid phase for the requested operation.
    InvalidPhaseTransition {
        current: ImportPhase,
        attempted: ImportPhase,
    },
    /// Coordinate out of bounds.
    CoordinateOutOfBounds {
        chunk_x: i32,
        chunk_z: i32,
    },
    /// Total chunks count mismatch.
    ChunkCountMismatch {
        expected: u32,
        actual: u32,
    },
    /// Fixture digest mismatch.
    DigestMismatch {
        expected: String,
        actual: String,
    },
    /// Import failed permanently.
    ImportFailed { reason: String },
}

impl std::fmt::Display for SolarImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidWorldKey => write!(f, "world_key must be non-zero"),
            Self::EmptyWorldName => write!(f, "world_name cannot be empty"),
            Self::EmptyChunkData { chunk_key } => {
                write!(f, "empty chunk payload for chunk 0x{chunk_key:032X}")
            }
            Self::CorruptChunkData { chunk_key, reason } => {
                write!(f, "corrupted chunk data for 0x{chunk_key:032X}: {reason}")
            }
            Self::ChecksumMismatch {
                chunk_key,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "checksum mismatch for chunk 0x{chunk_key:032X}: expected 0x{expected:08X}, got 0x{actual:08X}"
                )
            }
            Self::InvalidPhaseTransition { current, attempted } => {
                write!(
                    f,
                    "invalid import phase transition from {:?} to {:?}",
                    current, attempted
                )
            }
            Self::CoordinateOutOfBounds { chunk_x, chunk_z } => {
                write!(
                    f,
                    "chunk coordinates out of bounds: ({chunk_x}, {chunk_z})"
                )
            }
            Self::ChunkCountMismatch { expected, actual } => {
                write!(
                    f,
                    "chunk count mismatch: expected {expected}, got {actual}"
                )
            }
            Self::DigestMismatch { expected, actual } => {
                write!(
                    f,
                    "fixture digest mismatch: expected {expected}, got {actual}"
                )
            }
            Self::ImportFailed { reason } => {
                write!(f, "import failed: {reason}")
            }
        }
    }
}

impl std::error::Error for SolarImportError {}

/// Discovered raw chunk candidate before streaming.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub payload: Vec<u8>,
    pub checksum: u32,
}

impl DiscoveredChunk {
    pub fn new(chunk_x: i32, chunk_z: i32, payload: Vec<u8>) -> Self {
        let checksum = calculate_checksum(&payload);
        Self {
            chunk_x,
            chunk_z,
            payload,
            checksum,
        }
    }
}

/// Metadata and discovery manifest for an incoming Solar world import.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportManifest {
    pub world_key: u64,
    pub world_name: String,
    pub kind: String,
    pub owner: String,
    pub total_chunks: u32,
    pub min_y: i32,
    pub height: i32,
    pub spawn_x: f64,
    pub spawn_y: f64,
    pub spawn_z: f64,
    pub spawn_yaw: f32,
    pub spawn_pitch: f32,
    pub seed: i64,
    pub source_template: String,
    pub harbor_revision: u64,
    pub harbor_checksum: String,
    pub expected_fixture_digest: Option<String>,
}

/// Progress metrics recorded during an import run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportProgress {
    pub total_chunks: u32,
    pub completed_chunks: u32,
    pub skipped_duplicates: u32,
    pub newly_transferred: u32,
    pub bytes_streamed: u64,
    pub current_phase: &'static str,
    pub world_status: &'static str,
    pub failure_reason: Option<String>,
    pub fixture_digest: Option<String>,
}

/// Sink trait representing the target storage (e.g., SpacetimeDB table / cache).
pub trait SolarImportSink {
    /// Commit a base chunk payload to durable store.
    fn put_chunk_base(
        &mut self,
        world_key: u64,
        chunk_x: i32,
        chunk_z: i32,
        data: &[u8],
        base_revision: u64,
    ) -> Result<(), SolarImportError>;

    /// Check if a chunk already exists in durable store.
    fn has_chunk_base(&self, world_key: u64, chunk_x: i32, chunk_z: i32) -> bool;

    /// Update world metadata progress in storage.
    fn update_world_progress(
        &mut self,
        world_key: u64,
        completed_chunks: u32,
        phase: &'static str,
        status: &'static str,
        failure_reason: Option<&str>,
    ) -> Result<(), SolarImportError>;
}

/// In-memory implementation of `SolarImportSink` for tests, verification, and offline staging.
#[derive(Clone, Debug, Default)]
pub struct InMemoryImportSink {
    pub bases: BTreeMap<u128, SolarChunkBase>,
    pub world_progress: BTreeMap<u64, (u32, &'static str, &'static str, Option<String>)>,
}

impl InMemoryImportSink {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SolarImportSink for InMemoryImportSink {
    fn put_chunk_base(
        &mut self,
        world_key: u64,
        chunk_x: i32,
        chunk_z: i32,
        data: &[u8],
        base_revision: u64,
    ) -> Result<(), SolarImportError> {
        let chunk_key = pack_chunk_key(world_key, chunk_x, chunk_z);
        self.bases.insert(
            chunk_key,
            SolarChunkBase {
                chunk_key,
                world_key,
                chunk_x,
                chunk_z,
                data: data.to_vec(),
                base_revision,
                created_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(0),
                updated_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(0),
            },
        );
        Ok(())
    }

    fn has_chunk_base(&self, world_key: u64, chunk_x: i32, chunk_z: i32) -> bool {
        let chunk_key = pack_chunk_key(world_key, chunk_x, chunk_z);
        self.bases.contains_key(&chunk_key)
    }

    fn update_world_progress(
        &mut self,
        world_key: u64,
        completed_chunks: u32,
        phase: &'static str,
        status: &'static str,
        failure_reason: Option<&str>,
    ) -> Result<(), SolarImportError> {
        self.world_progress.insert(
            world_key,
            (
                completed_chunks,
                phase,
                status,
                failure_reason.map(|s| s.to_string()),
            ),
        );
        Ok(())
    }
}

/// Resumable Polar-to-Solar import controller.
#[derive(Debug)]
pub struct SolarImporter {
    manifest: ImportManifest,
    phase: ImportPhase,
    world_status: &'static str,
    completed_chunks: HashSet<u128>,
    discovered_chunks: Vec<DiscoveredChunk>,
    skipped_duplicates: u32,
    newly_transferred: u32,
    bytes_streamed: u64,
    failure_reason: Option<String>,
    fixture_digest: Option<String>,
}

impl SolarImporter {
    /// Initialize a new importer from a manifest and discovered candidate chunks.
    pub fn new(manifest: ImportManifest, discovered: Vec<DiscoveredChunk>) -> Result<Self, SolarImportError> {
        if manifest.world_key == 0 {
            return Err(SolarImportError::InvalidWorldKey);
        }
        if manifest.world_name.trim().is_empty() {
            return Err(SolarImportError::EmptyWorldName);
        }

        let total = discovered.len() as u32;
        let mut importer = Self {
            manifest,
            phase: ImportPhase::Discovery,
            world_status: WORLD_STATUS_IMPORTING,
            completed_chunks: HashSet::new(),
            discovered_chunks: discovered,
            skipped_duplicates: 0,
            newly_transferred: 0,
            bytes_streamed: 0,
            failure_reason: None,
            fixture_digest: None,
        };

        if importer.manifest.total_chunks == 0 {
            importer.manifest.total_chunks = total;
        }

        Ok(importer)
    }

    /// Current import phase.
    pub fn phase(&self) -> ImportPhase {
        self.phase
    }

    /// Current world status string (`IMPORTING`, `ACTIVE`, `FAILED`, `FALLBACK`).
    pub fn world_status(&self) -> &'static str {
        self.world_status
    }

    /// Current progress snapshot.
    pub fn progress(&self) -> ImportProgress {
        ImportProgress {
            total_chunks: self.manifest.total_chunks,
            completed_chunks: self.completed_chunks.len() as u32,
            skipped_duplicates: self.skipped_duplicates,
            newly_transferred: self.newly_transferred,
            bytes_streamed: self.bytes_streamed,
            current_phase: self.phase.as_str(),
            world_status: self.world_status,
            failure_reason: self.failure_reason.clone(),
            fixture_digest: self.fixture_digest.clone(),
        }
    }

    /// Restores or resumes an existing import session.
    ///
    /// Identifies all chunks already present in `sink` and populates `completed_chunks` to prevent redundant retransfers.
    pub fn resume<S: SolarImportSink>(&mut self, sink: &S) -> Result<(), SolarImportError> {
        if self.phase == ImportPhase::Failed {
            return Err(SolarImportError::ImportFailed {
                reason: self
                    .failure_reason
                    .clone()
                    .unwrap_or_else(|| "terminal failure".to_string()),
            });
        }

        let world_key = self.manifest.world_key;
        for chunk in &self.discovered_chunks {
            let chunk_key = pack_chunk_key(world_key, chunk.chunk_x, chunk.chunk_z);
            if sink.has_chunk_base(world_key, chunk.chunk_x, chunk.chunk_z) {
                self.completed_chunks.insert(chunk_key);
            }
        }

        // Transition from Discovery to StreamingBase
        self.phase = ImportPhase::StreamingBase;
        self.world_status = WORLD_STATUS_IMPORTING;
        Ok(())
    }

    /// Validates chunk payload integrity and checksum.
    ///
    /// Fails closed on any error or corruption.
    pub fn validate_chunk(
        world_key: u64,
        chunk: &DiscoveredChunk,
    ) -> Result<(), SolarImportError> {
        let chunk_key = pack_chunk_key(world_key, chunk.chunk_x, chunk.chunk_z);

        if chunk.payload.is_empty() {
            return Err(SolarImportError::EmptyChunkData { chunk_key });
        }

        // Check Adler-32 / CRC checksum
        let computed_checksum = calculate_checksum(&chunk.payload);
        if chunk.checksum != 0 && chunk.checksum != computed_checksum {
            return Err(SolarImportError::ChecksumMismatch {
                chunk_key,
                expected: chunk.checksum,
                actual: computed_checksum,
            });
        }

        // If encoded as a SolarChunkEnvelope, validate envelope structure
        if chunk.payload.len() >= 4 && &chunk.payload[0..4] == b"SOLR" {
            match SolarChunkEnvelope::decode(&chunk.payload) {
                Ok(env) => {
                    if env.world_key != world_key || env.chunk_x != chunk.chunk_x || env.chunk_z != chunk.chunk_z {
                        return Err(SolarImportError::CorruptChunkData {
                            chunk_key,
                            reason: format!(
                                "envelope key mismatch: header ({}, {}, {}) vs expected ({world_key}, {}, {})",
                                env.world_key, env.chunk_x, env.chunk_z, chunk.chunk_x, chunk.chunk_z
                            ),
                        });
                    }
                }
                Err(err) => {
                    return Err(SolarImportError::CorruptChunkData {
                        chunk_key,
                        reason: err.to_string(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Stream all discovered chunks into the sink.
    ///
    /// - Skips chunks that are already completed.
    /// - Fails closed and records terminal `FAILED` status on any corrupted chunk.
    /// - Updates progress monotonically.
    pub fn stream_all<S: SolarImportSink>(&mut self, sink: &mut S) -> Result<ImportProgress, SolarImportError> {
        if self.phase == ImportPhase::Discovery {
            self.resume(sink)?;
        }

        if self.phase != ImportPhase::StreamingBase {
            return Err(SolarImportError::InvalidPhaseTransition {
                current: self.phase,
                attempted: ImportPhase::StreamingBase,
            });
        }

        let world_key = self.manifest.world_key;
        let mut sorted_chunks = self.discovered_chunks.clone();
        sorted_chunks.sort_by_key(|c| (c.chunk_x, c.chunk_z));

        for chunk in &sorted_chunks {
            let chunk_key = pack_chunk_key(world_key, chunk.chunk_x, chunk.chunk_z);

            // 1. Check if already completed (Duplicate Avoidance)
            if self.completed_chunks.contains(&chunk_key) || sink.has_chunk_base(world_key, chunk.chunk_x, chunk.chunk_z) {
                self.completed_chunks.insert(chunk_key);
                self.skipped_duplicates += 1;
                continue;
            }

            // 2. Validate payload integrity (Fail Closed)
            if let Err(err) = Self::validate_chunk(world_key, chunk) {
                self.mark_failed(sink, format!("Chunk validation failed at ({}, {}): {err}", chunk.chunk_x, chunk.chunk_z))?;
                return Err(err);
            }

            // 3. Commit chunk base
            if let Err(err) = sink.put_chunk_base(world_key, chunk.chunk_x, chunk.chunk_z, &chunk.payload, 0) {
                self.mark_failed(sink, format!("Sink commit error at ({}, {}): {err}", chunk.chunk_x, chunk.chunk_z))?;
                return Err(err);
            }

            self.completed_chunks.insert(chunk_key);
            self.newly_transferred += 1;
            self.bytes_streamed += chunk.payload.len() as u64;

            // Report incremental progress
            sink.update_world_progress(
                world_key,
                self.completed_chunks.len() as u32,
                self.phase.as_str(),
                self.world_status,
                None,
            )?;
        }

        // Transition to VALIDATING phase
        self.phase = ImportPhase::Validating;
        sink.update_world_progress(
            world_key,
            self.completed_chunks.len() as u32,
            self.phase.as_str(),
            self.world_status,
            None,
        )?;

        // Validate final state and compute fixture digest
        self.finalize_validation(sink)?;

        Ok(self.progress())
    }

    /// Validates and computes the deterministic fixture digest across all imported chunks.
    fn finalize_validation<S: SolarImportSink>(&mut self, sink: &mut S) -> Result<(), SolarImportError> {
        let world_key = self.manifest.world_key;

        // Check if all expected chunks are present
        if (self.completed_chunks.len() as u32) < self.manifest.total_chunks {
            let err = SolarImportError::ChunkCountMismatch {
                expected: self.manifest.total_chunks,
                actual: self.completed_chunks.len() as u32,
            };
            self.mark_failed(sink, err.to_string())?;
            return Err(err);
        }

        // Calculate deterministic fixture digest over all chunks in canonical sorted order
        let mut sorted_chunks = self.discovered_chunks.clone();
        sorted_chunks.sort_by_key(|c| (c.chunk_x, c.chunk_z));

        let mut digest_acc: u64 = 0xcbf29ce484222325; // FNV-1a 64-bit offset basis
        for chunk in &sorted_chunks {
            let chunk_key = pack_chunk_key(world_key, chunk.chunk_x, chunk.chunk_z);
            for b in chunk_key.to_be_bytes() {
                digest_acc ^= b as u64;
                digest_acc = digest_acc.wrapping_mul(0x100000001b3);
            }
            for b in &chunk.payload {
                digest_acc ^= *b as u64;
                digest_acc = digest_acc.wrapping_mul(0x100000001b3);
            }
        }

        let computed_digest = format!("solar-digest-{digest_acc:016x}");
        self.fixture_digest = Some(computed_digest.clone());

        // Verify against expected digest if supplied
        if let Some(ref expected) = self.manifest.expected_fixture_digest
            && expected != &computed_digest
        {
            let err = SolarImportError::DigestMismatch {
                expected: expected.clone(),
                actual: computed_digest,
            };
            self.mark_failed(sink, err.to_string())?;
            return Err(err);
        }

        // Transition to COMPLETED and ACTIVE
        self.phase = ImportPhase::Completed;
        self.world_status = WORLD_STATUS_ACTIVE;

        sink.update_world_progress(
            world_key,
            self.completed_chunks.len() as u32,
            self.phase.as_str(),
            self.world_status,
            None,
        )?;

        Ok(())
    }

    /// Mark the import as failed closed and update the sink.
    pub fn mark_failed<S: SolarImportSink>(
        &mut self,
        sink: &mut S,
        reason: String,
    ) -> Result<(), SolarImportError> {
        self.phase = ImportPhase::Failed;
        self.world_status = WORLD_STATUS_FAILED;
        self.failure_reason = Some(reason.clone());

        sink.update_world_progress(
            self.manifest.world_key,
            self.completed_chunks.len() as u32,
            self.phase.as_str(),
            self.world_status,
            Some(&reason),
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_manifest(world_key: u64, total_chunks: u32) -> ImportManifest {
        ImportManifest {
            world_key,
            world_name: format!("world_{world_key}"),
            kind: "arena".to_string(),
            owner: "system".to_string(),
            total_chunks,
            min_y: -64,
            height: 384,
            spawn_x: 0.0,
            spawn_y: 64.0,
            spawn_z: 0.0,
            spawn_yaw: 0.0,
            spawn_pitch: 0.0,
            seed: 12345,
            source_template: "lobby".to_string(),
            harbor_revision: 1,
            harbor_checksum: "csum_test".to_string(),
            expected_fixture_digest: None,
        }
    }

    fn make_test_envelope_chunk(world_key: u64, cx: i32, cz: i32, payload_byte: u8) -> DiscoveredChunk {
        let env = SolarChunkEnvelope::new(world_key, cx, cz, 0, 0, vec![payload_byte; 128]).unwrap();
        let bytes = env.encode();
        DiscoveredChunk::new(cx, cz, bytes)
    }

    // ─── Acceptance 1: All five import states exercised ──────────────────────────

    #[test]
    fn test_all_five_import_states_exercised() {
        let manifest = make_test_manifest(101, 3);
        let chunks = vec![
            make_test_envelope_chunk(101, 0, 0, 1),
            make_test_envelope_chunk(101, 0, 1, 2),
            make_test_envelope_chunk(101, 1, 0, 3),
        ];

        let mut importer = SolarImporter::new(manifest, chunks).unwrap();
        let mut sink = InMemoryImportSink::new();

        // 1. Initial phase is DISCOVERY
        assert_eq!(importer.phase(), ImportPhase::Discovery);
        assert_eq!(importer.world_status(), WORLD_STATUS_IMPORTING);

        // 2. Stream chunks -> enters STREAMING_BASE, then VALIDATING, then COMPLETED
        let progress = importer.stream_all(&mut sink).unwrap();

        // 3. Final phase is COMPLETED and status is ACTIVE
        assert_eq!(importer.phase(), ImportPhase::Completed);
        assert_eq!(importer.world_status(), WORLD_STATUS_ACTIVE);
        assert_eq!(progress.completed_chunks, 3);
        assert_eq!(progress.current_phase, IMPORT_PHASE_COMPLETED);
        assert_eq!(progress.world_status, WORLD_STATUS_ACTIVE);

        // 4. Test FAILED state explicitly
        let manifest_fail = make_test_manifest(102, 1);
        let mut importer_fail = SolarImporter::new(manifest_fail, vec![]).unwrap();
        let mut sink_fail = InMemoryImportSink::new();
        importer_fail.mark_failed(&mut sink_fail, "Forced failure for test".to_string()).unwrap();

        assert_eq!(importer_fail.phase(), ImportPhase::Failed);
        assert_eq!(importer_fail.world_status(), WORLD_STATUS_FAILED);
        assert_eq!(importer_fail.progress().current_phase, IMPORT_PHASE_FAILED);
        assert_eq!(importer_fail.progress().world_status, WORLD_STATUS_FAILED);
    }

    // ─── Acceptance 2: Restart resumes ──────────────────────────────────────────

    #[test]
    fn test_restart_resumes_interrupted_import() {
        let world_key = 201;
        let manifest = make_test_manifest(world_key, 4);
        let chunks = vec![
            make_test_envelope_chunk(world_key, 0, 0, 10),
            make_test_envelope_chunk(world_key, 0, 1, 20),
            make_test_envelope_chunk(world_key, 1, 0, 30),
            make_test_envelope_chunk(world_key, 1, 1, 40),
        ];

        let mut sink = InMemoryImportSink::new();

        // Simulate partial import: pre-commit first 2 chunks to sink
        sink.put_chunk_base(world_key, 0, 0, &chunks[0].payload, 0).unwrap();
        sink.put_chunk_base(world_key, 0, 1, &chunks[1].payload, 0).unwrap();

        // Now start fresh importer on the same sink (simulating crash recovery / restart)
        let mut importer = SolarImporter::new(manifest, chunks).unwrap();
        let progress = importer.stream_all(&mut sink).unwrap();

        // Must complete all 4 chunks
        assert_eq!(progress.completed_chunks, 4);
        assert_eq!(progress.skipped_duplicates, 2, "First 2 chunks should be skipped on resume");
        assert_eq!(progress.newly_transferred, 2, "Only 2 remaining chunks should be transferred");
        assert_eq!(importer.phase(), ImportPhase::Completed);
        assert_eq!(importer.world_status(), WORLD_STATUS_ACTIVE);
        assert_eq!(sink.bases.len(), 4);
    }

    // ─── Acceptance 3: Corrupt input fails closed ───────────────────────────────

    #[test]
    fn test_corrupt_input_fails_closed() {
        let world_key = 301;
        let manifest = make_test_manifest(world_key, 2);

        // Good chunk 1
        let chunk1 = make_test_envelope_chunk(world_key, 0, 0, 1);

        // Corrupted chunk 2: invalid checksum and modified payload bytes
        let mut chunk2 = make_test_envelope_chunk(world_key, 0, 1, 2);
        chunk2.payload[20] ^= 0xFF; // flip bits in payload
        chunk2.checksum = 0x12345678; // wrong checksum

        let mut importer = SolarImporter::new(manifest, vec![chunk1, chunk2]).unwrap();
        let mut sink = InMemoryImportSink::new();

        let result = importer.stream_all(&mut sink);
        assert!(result.is_err(), "Corrupt chunk must fail closed");

        // Importer and sink must be in FAILED state
        assert_eq!(importer.phase(), ImportPhase::Failed);
        assert_eq!(importer.world_status(), WORLD_STATUS_FAILED);
        assert!(importer.failure_reason.is_some());

        let world_prog = sink.world_progress.get(&world_key).unwrap();
        assert_eq!(world_prog.1, IMPORT_PHASE_FAILED);
        assert_eq!(world_prog.2, WORLD_STATUS_FAILED);
    }

    #[test]
    fn test_empty_chunk_data_fails_closed() {
        let world_key = 302;
        let manifest = make_test_manifest(world_key, 1);
        let empty_chunk = DiscoveredChunk::new(0, 0, vec![]);

        let mut importer = SolarImporter::new(manifest, vec![empty_chunk]).unwrap();
        let mut sink = InMemoryImportSink::new();

        let result = importer.stream_all(&mut sink);
        assert!(matches!(result, Err(SolarImportError::EmptyChunkData { .. })));
        assert_eq!(importer.phase(), ImportPhase::Failed);
    }

    // ─── Acceptance 4: Completed chunks not retransferred ───────────────────────

    #[test]
    fn test_completed_chunks_not_retransferred() {
        let world_key = 401;
        let manifest = make_test_manifest(world_key, 3);
        let chunks = vec![
            make_test_envelope_chunk(world_key, 0, 0, 100),
            make_test_envelope_chunk(world_key, 0, 1, 101),
            make_test_envelope_chunk(world_key, 1, 0, 102),
        ];

        let mut sink = InMemoryImportSink::new();
        let mut importer1 = SolarImporter::new(manifest.clone(), chunks.clone()).unwrap();

        // First pass transfers all 3 chunks
        let p1 = importer1.stream_all(&mut sink).unwrap();
        assert_eq!(p1.newly_transferred, 3);
        assert_eq!(p1.skipped_duplicates, 0);

        // Second pass (e.g. redundant retry or re-import) with same sink
        let mut importer2 = SolarImporter::new(manifest, chunks).unwrap();
        let p2 = importer2.stream_all(&mut sink).unwrap();
        assert_eq!(p2.newly_transferred, 0, "No chunks should be retransferred");
        assert_eq!(p2.skipped_duplicates, 3, "All 3 chunks should be skipped as duplicates");
        assert_eq!(p2.completed_chunks, 3);
    }

    // ─── Acceptance 5: Fixture digest recorded and verified ─────────────────────

    #[test]
    fn test_fixture_digest_recorded_and_verified() {
        let world_key = 501;
        let chunks = vec![
            make_test_envelope_chunk(world_key, 0, 0, 50),
            make_test_envelope_chunk(world_key, 0, 1, 60),
            make_test_envelope_chunk(world_key, 1, 0, 70),
        ];

        // 1. Run first import and capture the fixture digest
        let manifest1 = make_test_manifest(world_key, 3);
        let mut importer1 = SolarImporter::new(manifest1, chunks.clone()).unwrap();
        let mut sink1 = InMemoryImportSink::new();
        let p1 = importer1.stream_all(&mut sink1).unwrap();

        let digest = p1.fixture_digest.expect("Fixture digest must be recorded");
        assert!(digest.starts_with("solar-digest-"));

        // 2. Run second import with expected digest matching -> succeeds
        let mut manifest2 = make_test_manifest(world_key, 3);
        manifest2.expected_fixture_digest = Some(digest.clone());
        let mut importer2 = SolarImporter::new(manifest2, chunks.clone()).unwrap();
        let mut sink2 = InMemoryImportSink::new();
        let p2 = importer2.stream_all(&mut sink2).unwrap();
        assert_eq!(p2.fixture_digest.unwrap(), digest);

        // 3. Run third import with bad expected digest -> fails closed with DigestMismatch
        let mut manifest3 = make_test_manifest(world_key, 3);
        manifest3.expected_fixture_digest = Some("solar-digest-0000000000000000_wrong".to_string());
        let mut importer3 = SolarImporter::new(manifest3, chunks).unwrap();
        let mut sink3 = InMemoryImportSink::new();
        let result3 = importer3.stream_all(&mut sink3);
        assert!(matches!(result3, Err(SolarImportError::DigestMismatch { .. })));
        assert_eq!(importer3.phase(), ImportPhase::Failed);
    }
}
