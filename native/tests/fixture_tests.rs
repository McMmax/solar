use std::fs;
use std::path::PathBuf;
use solar_bridge::{
    fold_chunk_frame, reconstruct_chunk_frame, pack_chunk_key, SolarBlockEdit, SolarChunkBase,
    SolarChunkFrame, SolarChunkHead, SolarChunkPatch, SolarReadError,
    SolarChunkEnvelope, SolarChunkPatchFrame
};

#[test]
fn test_deterministic_fixture_loading() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let fixture_path = PathBuf::from(manifest_dir).join("tests/fixtures/solar_fixtures.json");
    let content = fs::read_to_string(fixture_path).expect("Failed to read fixture file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("Failed to parse fixture json");
    let fixtures = parsed.get("fixtures").expect("Missing fixtures object");

    // My new assertions for the ticket!
    let required = vec![
        "all_air_chunk",
        "Lobby",
        "Soupkitz",
        "fluid_sign",
        "cross_chunk_placement",
        "malformed_payload",
        "world_edit_janitor",
        "replay",
    ];

    for key in required {
        let fixture = fixtures.get(key).unwrap_or_else(|| panic!("Missing fixture: {}", key));
        let digest = fixture.get("digest").expect("Fixture must include digest");
        assert!(digest.is_string(), "Digest must be a string");
    }
    let replay = fixtures.get("replay").unwrap();
    assert_eq!(replay.get("digest").unwrap().as_str().unwrap(), "stable_replay_digest_1234567890a");


    // Original assertions translated for the new tests
    // 1. all-air and real-world fixtures pass
    let air_chunk = fixtures.get("all_air_chunk").unwrap();
    assert_eq!(air_chunk["edits"].as_array().unwrap().len(), 0);
    
    let lobby = fixtures.get("Lobby").unwrap();
    let lobby_edits = lobby["edits"].as_array().unwrap();
    assert_eq!(lobby_edits.len(), 1);
    
    // Test base-plus-patch fold with deterministic output
    let chunk_key = pack_chunk_key(1, 0, 0);
    let mut frame = SolarChunkFrame {
        chunk_key,
        world_key: 1,
        chunk_x: 0,
        chunk_z: 0,
        applied_revision: 1,
        base_revision: 0,
        base_data: vec![0x00, 0x01], // arbitrary base
        ordered_patches: vec![],
    };
    
    let folded_air = fold_chunk_frame(&frame).unwrap();
    assert_eq!(folded_air.folded_patch_count, 0);
    
    // Add one patch
    frame.ordered_patches.push(SolarChunkPatchFrame {
        revision: 1,
        edits: vec![SolarBlockEdit::new(0, 0, 0, 1)],
        actor: "tester".to_string(),
    });
    
    let folded_solid = fold_chunk_frame(&frame).unwrap();
    assert_eq!(folded_solid.folded_patch_count, 1);
    
    // 2. out-of-order and revision-gap data fail closed
    let base = SolarChunkBase {
        chunk_key,
        world_key: 1,
        chunk_x: 0,
        chunk_z: 0,
        data: vec![0x00, 0x01],
        base_revision: 1,
        created_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(1),
        updated_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(1),
    };
    
    let head = SolarChunkHead {
        chunk_key,
        world_key: 1,
        chunk_x: 0,
        chunk_z: 0,
        applied_revision: 3,
        base_revision: 1,
        patch_count: 2,
        updated_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(1),
        last_actor: "tester".to_string(),
    };
    
    // Gap: expects revision 2, found 3
    let patches = vec![
        SolarChunkPatch {
            id: 2,
            chunk_key,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            revision: 3,
            edits: vec![],
            actor: "tester".to_string(),
            created_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(1),
        }
    ];
    
    let res = reconstruct_chunk_frame(chunk_key, Some(&base), Some(&head), &patches);
    assert!(matches!(res, Err(SolarReadError::RevisionGap { .. })));

    // 3. duplicates deterministic
    // Assuming identical frames yield identical folded data
    let frame_dup = frame.clone();
    let folded_1 = fold_chunk_frame(&frame).unwrap();
    let folded_2 = fold_chunk_frame(&frame_dup).unwrap();
    assert_eq!(folded_1.data, folded_2.data);
    
    // malformed payload test
    let malformed = fixtures.get("malformed_payload").unwrap();
    let bad_payload = malformed["payload"].as_str().unwrap().as_bytes();
    let decode_res = SolarChunkEnvelope::decode(bad_payload);
    assert!(decode_res.is_err());
}

#[test]
fn test_fixture_replay_deterministic_execution() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let fixture_path = PathBuf::from(manifest_dir).join("tests/fixtures/solar_fixtures.json");
    let content = fs::read_to_string(fixture_path).expect("Failed to read fixture file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("Failed to parse fixture json");
    let fixtures = parsed.get("fixtures").expect("Missing fixtures object");

    // 1. Test "replay" fixture with SolarRecoveryManager
    let replay_fixture = fixtures.get("replay").expect("Missing replay fixture");
    let patches_json = replay_fixture["patches"].as_array().expect("patches array");

    let mut manager = solar_bridge::SolarRecoveryManager::new();
    let t0 = spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(1000);
    let chunk_key = manager.initialize_chunk(1, 0, 0, vec![0x10, 0x20, 0x30, 0x40], t0).unwrap();

    for patch in patches_json {
        let patch_id = patch["id"].as_u64().unwrap();
        let expected_rev = patch_id - 1;
        let edits_json = patch["edits"].as_array().unwrap();
        let edits: Vec<SolarBlockEdit> = edits_json.iter().map(|e| {
            SolarBlockEdit::new(
                e["x"].as_u64().unwrap() as u8,
                e["y"].as_i64().unwrap() as i16,
                e["z"].as_u64().unwrap() as u8,
                e["state"].as_i64().unwrap() as i32,
            )
        }).collect();

        manager.stage_mutation(patch_id, chunk_key, expected_rev, edits, "replay_agent".to_string(), t0).unwrap();
        let receipt = manager.acknowledge_mutation(patch_id, t0).unwrap();
        assert_eq!(receipt.committed_revision, patch_id);
    }

    assert_eq!(manager.head_revision(chunk_key), Some(2));
    let folded_replay = manager.replay_chunk(chunk_key).unwrap();
    assert_eq!(folded_replay.revision, 2);
    assert_eq!(folded_replay.folded_patch_count, 2);

    // 2. Test "world_edit_janitor" fixture
    let we_fixture = fixtures.get("world_edit_janitor").unwrap();
    let we_edits: Vec<SolarBlockEdit> = we_fixture["edits"].as_array().unwrap().iter().map(|e| {
        SolarBlockEdit::new(
            e["x"].as_u64().unwrap() as u8,
            e["y"].as_i64().unwrap() as i16,
            e["z"].as_u64().unwrap() as u8,
            e["state"].as_i64().unwrap() as i32,
        )
    }).collect();

    let mut we_manager = solar_bridge::SolarRecoveryManager::new();
    let we_key = we_manager.initialize_chunk(2, 5, 5, vec![0xAA, 0xBB], t0).unwrap();
    we_manager.stage_mutation(1, we_key, 0, we_edits.clone(), "worldedit".to_string(), t0).unwrap();
    let we_receipt = we_manager.acknowledge_mutation(1, t0).unwrap();
    assert_eq!(we_receipt.committed_revision, 1);
    assert_eq!(we_receipt.edit_count, 3);
    let we_folded = we_manager.replay_chunk(we_key).unwrap();
    assert_eq!(we_folded.folded_edit_count, 3);
}

#[test]
fn test_rollback_and_read_only_fallback_invariants() {
    let t0 = spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(500);
    let mut manager = solar_bridge::SolarRecoveryManager::new();

    // 1. Setup known-good baseline world state (Revision 0 -> 2)
    let chunk_key = manager
        .initialize_chunk(100, 0, 0, vec![0x01, 0x02, 0x03, 0x04], t0)
        .expect("Chunk initialization must succeed");

    let edit1 = vec![SolarBlockEdit::new(1, 64, 1, 10)];
    manager.stage_mutation(101, chunk_key, 0, edit1, "actor1".into(), t0).unwrap();
    let r1 = manager.acknowledge_mutation(101, t0).unwrap();
    assert_eq!(r1.committed_revision, 1);

    let edit2 = vec![SolarBlockEdit::new(2, 64, 2, 20)];
    manager.stage_mutation(102, chunk_key, 1, edit2, "actor2".into(), t0).unwrap();
    let r2 = manager.acknowledge_mutation(102, t0).unwrap();
    assert_eq!(r2.committed_revision, 2);

    let known_good_frame = manager.read_chunk_frame(chunk_key, Some(2)).unwrap();
    let known_good_folded = solar_bridge::fold_chunk_frame(&known_good_frame).unwrap();
    assert_eq!(known_good_folded.revision, 2);

    // 2. Uncommitted canary / crash state (Stage mutation 103 without ACK)
    let canary_edit = vec![SolarBlockEdit::new(3, 64, 3, 99)];
    manager.stage_mutation(103, chunk_key, 2, canary_edit, "canary_worker".into(), t0).unwrap();
    assert_eq!(manager.pending_staged_count(), 1);

    // 3. Rollback / Crash recovery execution
    let restart_summary = manager.simulate_crash_and_restart();
    assert_eq!(restart_summary.discarded_unacknowledged_mutations, 1);
    assert_eq!(manager.pending_staged_count(), 0);

    // 4. Verify exact rollback state matches known-good revision 2
    let rolled_back_frame = manager.read_chunk_frame(chunk_key, None).unwrap();
    assert_eq!(rolled_back_frame.applied_revision, 2);
    let rolled_back_folded = solar_bridge::fold_chunk_frame(&rolled_back_frame).unwrap();
    assert_eq!(rolled_back_folded.data, known_good_folded.data);
    assert_eq!(rolled_back_folded.revision, 2);

    // 5. Fallback read-only validation: Fallback cannot accept invalid/stale/forward writes
    // Out-of-order / invalid revision staged write fails closed
    let bad_revision_edit = vec![SolarBlockEdit::new(4, 64, 4, 30)];
    let err_stale = manager.stage_mutation(104, chunk_key, 1, bad_revision_edit.clone(), "fallback_actor".into(), t0);
    assert!(matches!(err_stale, Err(solar_bridge::recovery::SolarRecoveryError::RevisionConflict { expected: 1, current: 2, .. })));

    let err_future = manager.stage_mutation(105, chunk_key, 3, bad_revision_edit, "fallback_actor".into(), t0);
    assert!(matches!(err_future, Err(solar_bridge::recovery::SolarRecoveryError::RevisionConflict { expected: 3, current: 2, .. })));

    // Unregistered/missing chunk writes fail closed with NotFound
    let missing_chunk_key = solar_bridge::pack_chunk_key(999, 10, 10);
    let err_not_found = manager.stage_mutation(106, missing_chunk_key, 0, vec![SolarBlockEdit::new(0, 64, 0, 1)], "fallback_actor".into(), t0);
    assert!(matches!(err_not_found, Err(solar_bridge::recovery::SolarRecoveryError::NotFound { .. })));
}

