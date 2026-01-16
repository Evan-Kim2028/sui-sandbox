//! Comprehensive tests for natives.rs - Mock crypto, tx_context, dynamic fields
//!
//! Test coverage areas:
//! - MockClock: advancing timestamps, reset, boundary conditions
//! - MockRandom: deterministic output, seed variations, reproducibility
//! - EventStore: emit, query, clear, threading safety
//! - MockNativeState: ID generation, combined operations
//! - Native function behavior: tx_context, crypto mocks, event emission

use std::sync::Arc;
use std::thread;

use sui_move_interface_extractor::benchmark::natives::{
    EventStore, MockClock, MockNativeState, MockRandom,
};

// =============================================================================
// MockClock Tests
// =============================================================================

mod mock_clock_tests {
    use super::*;

    #[test]
    fn test_clock_default_base_timestamp() {
        let clock = MockClock::new();
        assert_eq!(clock.base_ms, MockClock::DEFAULT_BASE_MS);
        assert_eq!(clock.tick_ms, MockClock::DEFAULT_TICK_MS);
    }

    #[test]
    fn test_clock_with_custom_base() {
        let custom_base = 1000000;
        let clock = MockClock::with_base(custom_base);
        assert_eq!(clock.base_ms, custom_base);
    }

    #[test]
    fn test_clock_advances_on_each_access() {
        let clock = MockClock::new();

        let t1 = clock.timestamp_ms();
        let t2 = clock.timestamp_ms();
        let t3 = clock.timestamp_ms();

        assert!(t2 > t1, "timestamp should advance");
        assert!(t3 > t2, "timestamp should continue advancing");
        assert_eq!(t2 - t1, MockClock::DEFAULT_TICK_MS);
        assert_eq!(t3 - t2, MockClock::DEFAULT_TICK_MS);
    }

    #[test]
    fn test_clock_peek_does_not_advance() {
        let clock = MockClock::new();

        let p1 = clock.peek_timestamp_ms();
        let p2 = clock.peek_timestamp_ms();
        let p3 = clock.peek_timestamp_ms();

        assert_eq!(p1, p2, "peek should not advance");
        assert_eq!(p2, p3, "peek should not advance");
    }

    #[test]
    fn test_clock_peek_after_advance() {
        let clock = MockClock::new();

        let _ = clock.timestamp_ms(); // advance
        let _ = clock.timestamp_ms(); // advance again

        let peek_val = clock.peek_timestamp_ms();
        let next_val = clock.timestamp_ms();

        assert_eq!(peek_val, next_val, "peek should match next timestamp");
    }

    #[test]
    fn test_clock_reset() {
        let clock = MockClock::new();

        let initial = clock.timestamp_ms();
        let _ = clock.timestamp_ms();
        let _ = clock.timestamp_ms();

        clock.reset();

        let after_reset = clock.timestamp_ms();
        assert_eq!(initial, after_reset, "reset should return to initial state");
    }

    #[test]
    fn test_clock_many_accesses() {
        let clock = MockClock::new();

        for i in 0..1000u64 {
            let ts = clock.timestamp_ms();
            let expected = MockClock::DEFAULT_BASE_MS + (i * MockClock::DEFAULT_TICK_MS);
            assert_eq!(ts, expected, "timestamp {i} should match expected");
        }
    }

    #[test]
    fn test_clock_thread_safety() {
        let clock = Arc::new(MockClock::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let clock_clone = clock.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let _ = clock_clone.timestamp_ms();
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        // After 10 threads * 100 accesses = 1000 total advances
        let final_peek = clock.peek_timestamp_ms();
        let expected = MockClock::DEFAULT_BASE_MS + (1000 * MockClock::DEFAULT_TICK_MS);
        assert_eq!(final_peek, expected, "all accesses should be counted");
    }

    #[test]
    fn test_clock_zero_base() {
        let clock = MockClock::with_base(0);

        let t1 = clock.timestamp_ms();
        assert_eq!(t1, 0, "first timestamp with zero base should be 0");

        let t2 = clock.timestamp_ms();
        assert_eq!(t2, MockClock::DEFAULT_TICK_MS);
    }

    #[test]
    fn test_clock_near_overflow() {
        let clock = MockClock::with_base(u64::MAX - 10000);

        // Should not panic on access
        let t1 = clock.timestamp_ms();
        assert!(t1 >= u64::MAX - 10000);
    }
}

// =============================================================================
// MockRandom Tests
// =============================================================================

mod mock_random_tests {
    use super::*;

    #[test]
    fn test_random_default_seed() {
        let random = MockRandom::new();
        let bytes = random.next_bytes(32);
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn test_random_custom_seed() {
        let seed = [1u8; 32];
        let random = MockRandom::with_seed(seed);
        let bytes = random.next_bytes(32);
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn test_random_is_deterministic() {
        let seed = [42u8; 32];

        let random1 = MockRandom::with_seed(seed);
        let random2 = MockRandom::with_seed(seed);

        let bytes1 = random1.next_bytes(64);
        let bytes2 = random2.next_bytes(64);

        assert_eq!(bytes1, bytes2, "same seed should produce same output");
    }

    #[test]
    fn test_random_different_seeds_differ() {
        let random1 = MockRandom::with_seed([0u8; 32]);
        let random2 = MockRandom::with_seed([1u8; 32]);

        let bytes1 = random1.next_bytes(32);
        let bytes2 = random2.next_bytes(32);

        assert_ne!(
            bytes1, bytes2,
            "different seeds should produce different output"
        );
    }

    #[test]
    fn test_random_sequence_changes() {
        let random = MockRandom::new();

        let bytes1 = random.next_bytes(32);
        let bytes2 = random.next_bytes(32);
        let bytes3 = random.next_bytes(32);

        assert_ne!(bytes1, bytes2, "sequential calls should differ");
        assert_ne!(bytes2, bytes3, "sequential calls should differ");
        assert_ne!(bytes1, bytes3, "sequential calls should differ");
    }

    #[test]
    fn test_random_reset_reproduces_sequence() {
        let random = MockRandom::new();

        let seq1_a = random.next_bytes(32);
        let seq1_b = random.next_bytes(32);

        random.reset();

        let seq2_a = random.next_bytes(32);
        let seq2_b = random.next_bytes(32);

        assert_eq!(seq1_a, seq2_a, "reset should reproduce first value");
        assert_eq!(seq1_b, seq2_b, "reset should reproduce second value");
    }

    #[test]
    fn test_random_various_lengths() {
        let random = MockRandom::new();

        for len in [0, 1, 16, 32, 64, 100, 256] {
            let bytes = random.next_bytes(len);
            assert_eq!(bytes.len(), len, "should return requested length: {len}");
        }
    }

    #[test]
    fn test_random_zero_length() {
        let random = MockRandom::new();
        let bytes = random.next_bytes(0);
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_random_large_output() {
        let random = MockRandom::new();
        let bytes = random.next_bytes(1000);
        assert_eq!(bytes.len(), 1000);
    }

    #[test]
    fn test_random_thread_safety() {
        let random = Arc::new(MockRandom::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let random_clone = random.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let _ = random_clone.next_bytes(32);
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        // Just verify no panic occurred
    }
}

// =============================================================================
// EventStore Tests
// =============================================================================

mod event_store_tests {
    use super::*;

    #[test]
    fn test_event_store_empty() {
        let store = EventStore::new();
        let events = store.get_events();
        assert!(events.is_empty());
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_event_store_emit_single() {
        let store = EventStore::new();

        store.emit("0x2::coin::CoinCreated".to_string(), vec![1, 2, 3]);

        let events = store.get_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].type_tag, "0x2::coin::CoinCreated");
        assert_eq!(events[0].data, vec![1, 2, 3]);
        assert_eq!(events[0].sequence, 0);
    }

    #[test]
    fn test_event_store_emit_multiple() {
        let store = EventStore::new();

        store.emit("Event1".to_string(), vec![1]);
        store.emit("Event2".to_string(), vec![2]);
        store.emit("Event3".to_string(), vec![3]);

        let events = store.get_events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].sequence, 0);
        assert_eq!(events[1].sequence, 1);
        assert_eq!(events[2].sequence, 2);
    }

    #[test]
    fn test_event_store_count() {
        let store = EventStore::new();

        assert_eq!(store.count(), 0);
        store.emit("E1".to_string(), vec![]);
        assert_eq!(store.count(), 1);
        store.emit("E2".to_string(), vec![]);
        assert_eq!(store.count(), 2);
    }

    #[test]
    fn test_event_store_clear() {
        let store = EventStore::new();

        store.emit("Event1".to_string(), vec![1]);
        store.emit("Event2".to_string(), vec![2]);

        assert_eq!(store.count(), 2);

        store.clear();

        assert_eq!(store.count(), 0);
        assert!(store.get_events().is_empty());
    }

    #[test]
    fn test_event_store_sequence_resets_on_clear() {
        let store = EventStore::new();

        store.emit("E1".to_string(), vec![]);
        store.emit("E2".to_string(), vec![]);
        store.clear();
        store.emit("E3".to_string(), vec![]);

        let events = store.get_events();
        assert_eq!(events[0].sequence, 0, "sequence should reset after clear");
    }

    #[test]
    fn test_event_store_get_by_type_prefix() {
        let store = EventStore::new();

        store.emit("0x2::coin::Created".to_string(), vec![1]);
        store.emit("0x2::coin::Destroyed".to_string(), vec![2]);
        store.emit("0x2::balance::Supply".to_string(), vec![3]);
        store.emit("0x2::coin::Transfer".to_string(), vec![4]);

        let coin_events = store.get_events_by_type("0x2::coin::");
        assert_eq!(coin_events.len(), 3);

        let balance_events = store.get_events_by_type("0x2::balance::");
        assert_eq!(balance_events.len(), 1);

        let no_events = store.get_events_by_type("0x999::");
        assert!(no_events.is_empty());
    }

    #[test]
    fn test_event_store_empty_type() {
        let store = EventStore::new();
        store.emit("".to_string(), vec![1, 2, 3]);

        let events = store.get_events();
        assert_eq!(events.len(), 1);
        assert!(events[0].type_tag.is_empty());
    }

    #[test]
    fn test_event_store_empty_data() {
        let store = EventStore::new();
        store.emit("SomeEvent".to_string(), vec![]);

        let events = store.get_events();
        assert_eq!(events.len(), 1);
        assert!(events[0].data.is_empty());
    }

    #[test]
    fn test_event_store_large_data() {
        let store = EventStore::new();
        let large_data = vec![42u8; 10000];
        store.emit("LargeEvent".to_string(), large_data.clone());

        let events = store.get_events();
        assert_eq!(events[0].data.len(), 10000);
        assert_eq!(events[0].data, large_data);
    }

    #[test]
    fn test_event_store_thread_safety() {
        let store = Arc::new(EventStore::new());
        let mut handles = vec![];

        for i in 0..10 {
            let store_clone = store.clone();
            handles.push(thread::spawn(move || {
                for j in 0..100 {
                    store_clone.emit(format!("Event_{}_{}", i, j), vec![i as u8, j as u8]);
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        // 10 threads * 100 events = 1000 total
        assert_eq!(store.count(), 1000);
        assert_eq!(store.get_events().len(), 1000);
    }
}

// =============================================================================
// MockNativeState Tests
// =============================================================================

mod mock_native_state_tests {
    use super::*;
    use move_core_types::account_address::AccountAddress;

    #[test]
    fn test_state_default_values() {
        let state = MockNativeState::new();

        assert_eq!(state.sender, AccountAddress::ZERO);
        assert_eq!(state.epoch, 0);
        assert_eq!(state.epoch_timestamp_ms, MockClock::DEFAULT_BASE_MS);
        assert_eq!(state.ids_created(), 0);
    }

    #[test]
    fn test_state_with_random_seed() {
        let seed = [77u8; 32];
        let state = MockNativeState::with_random_seed(seed);

        // Verify it works
        let bytes1 = state.random_bytes(32);
        let bytes2 = state.random_bytes(32);
        assert_ne!(bytes1, bytes2);
    }

    #[test]
    fn test_state_fresh_id_uniqueness() {
        let state = MockNativeState::new();

        let id1 = state.fresh_id();
        let id2 = state.fresh_id();
        let id3 = state.fresh_id();

        assert_ne!(id1, id2, "IDs should be unique");
        assert_ne!(id2, id3, "IDs should be unique");
        assert_ne!(id1, id3, "IDs should be unique");
    }

    #[test]
    fn test_state_ids_created_counter() {
        let state = MockNativeState::new();

        assert_eq!(state.ids_created(), 0);

        let _ = state.fresh_id();
        assert_eq!(state.ids_created(), 1);

        let _ = state.fresh_id();
        let _ = state.fresh_id();
        assert_eq!(state.ids_created(), 3);
    }

    #[test]
    fn test_state_clock_timestamp() {
        let state = MockNativeState::new();

        let t1 = state.clock_timestamp_ms();
        let t2 = state.clock_timestamp_ms();

        assert!(t2 > t1, "clock should advance");
    }

    #[test]
    fn test_state_random_bytes() {
        let state = MockNativeState::new();

        let bytes = state.random_bytes(64);
        assert_eq!(bytes.len(), 64);
    }

    #[test]
    fn test_state_events() {
        let state = MockNativeState::new();

        assert!(state.get_events().is_empty());

        state.events.emit("TestEvent".to_string(), vec![1, 2, 3]);

        assert_eq!(state.get_events().len(), 1);

        state.clear_events();
        assert!(state.get_events().is_empty());
    }

    #[test]
    fn test_state_get_events_by_type() {
        let state = MockNativeState::new();

        state.events.emit("0x2::coin::A".to_string(), vec![]);
        state.events.emit("0x2::coin::B".to_string(), vec![]);
        state.events.emit("0x2::other::C".to_string(), vec![]);

        let coin_events = state.get_events_by_type("0x2::coin::");
        assert_eq!(coin_events.len(), 2);
    }

    #[test]
    fn test_state_thread_safety_fresh_id() {
        let state = Arc::new(MockNativeState::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let state_clone = state.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let _ = state_clone.fresh_id();
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        // 10 threads * 100 IDs = 1000 total
        assert_eq!(state.ids_created(), 1000);
    }

    #[test]
    fn test_state_fresh_id_sequential_pattern() {
        let state = MockNativeState::new();

        // IDs should follow a sequential pattern in the last 8 bytes
        let id1 = state.fresh_id();
        let id2 = state.fresh_id();

        let bytes1 = id1.into_bytes();
        let bytes2 = id2.into_bytes();

        // First 24 bytes should be zero
        assert_eq!(&bytes1[0..24], &[0u8; 24]);
        assert_eq!(&bytes2[0..24], &[0u8; 24]);

        // Last 8 bytes contain the counter
        let counter1 = u64::from_le_bytes(bytes1[24..32].try_into().unwrap());
        let counter2 = u64::from_le_bytes(bytes2[24..32].try_into().unwrap());

        assert_eq!(counter1, 0);
        assert_eq!(counter2, 1);
    }
}

// =============================================================================
// Crypto Mock Behavior Tests
// =============================================================================

mod crypto_mock_behavior_tests {
    // Note: These tests verify the EXPECTED behavior of crypto mocks
    // (they always return success), which is intentional for type inhabitation.

    #[test]
    fn test_crypto_mocks_documented_behavior() {
        // This test documents the intentional behavior:
        // - bls12381_min_pk_verify returns true
        // - secp256k1_verify returns true
        // - ed25519_verify returns true
        // - etc.
        //
        // This is by design for type inhabitation testing where we want
        // execution to complete, not to verify cryptographic correctness.

        // The actual native functions are tested through VM execution in vm_tests.rs
        // This test serves as documentation - no assertions needed.
    }
}

// =============================================================================
// Edge Cases and Error Handling
// =============================================================================

mod edge_case_tests {
    use super::*;

    #[test]
    fn test_many_sequential_operations() {
        let state = MockNativeState::new();

        // Perform many operations in sequence
        for _ in 0..10000 {
            let _ = state.fresh_id();
            let _ = state.clock_timestamp_ms();
            let _ = state.random_bytes(32);
        }

        assert_eq!(state.ids_created(), 10000);
    }

    #[test]
    fn test_concurrent_mixed_operations() {
        let state = Arc::new(MockNativeState::new());
        let mut handles = vec![];

        // Thread 1: Generate IDs
        let state1 = state.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..1000 {
                let _ = state1.fresh_id();
            }
        }));

        // Thread 2: Clock accesses
        let state2 = state.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..1000 {
                let _ = state2.clock_timestamp_ms();
            }
        }));

        // Thread 3: Random bytes
        let state3 = state.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..1000 {
                let _ = state3.random_bytes(64);
            }
        }));

        // Thread 4: Events
        let state4 = state.clone();
        handles.push(thread::spawn(move || {
            for i in 0..1000 {
                state4.events.emit(format!("Event{}", i), vec![i as u8]);
            }
        }));

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        assert_eq!(state.ids_created(), 1000);
        assert_eq!(state.events.count(), 1000);
    }
}
