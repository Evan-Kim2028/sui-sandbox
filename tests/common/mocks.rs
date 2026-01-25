//! Mock object creation utilities.
//!
//! Provides helpers for creating mock coins and other test data
//! that simulate Sui on-chain structures.

use move_core_types::account_address::AccountAddress;

/// Size of a mock coin in bytes (32-byte UID + 8-byte balance).
const MOCK_COIN_SIZE: usize = 40;

/// Create a mock coin with the given ID and balance.
///
/// The mock coin structure matches the Sui Coin layout:
/// - First 32 bytes: UID (object ID)
/// - Next 8 bytes: balance (u64, little-endian)
pub fn create_mock_coin(id: AccountAddress, balance: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(MOCK_COIN_SIZE);
    bytes.extend_from_slice(id.as_ref()); // 32-byte UID
    bytes.extend_from_slice(&balance.to_le_bytes()); // 8-byte balance
    bytes
}

/// Extract the balance from mock coin bytes.
pub fn get_coin_balance(bytes: &[u8]) -> u64 {
    if bytes.len() >= MOCK_COIN_SIZE {
        u64::from_le_bytes(bytes[32..40].try_into().unwrap())
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_mock_coin() {
        let id = AccountAddress::from_hex_literal("0x1234").unwrap();
        let coin = create_mock_coin(id, 1000);

        assert_eq!(coin.len(), MOCK_COIN_SIZE);
        assert_eq!(get_coin_balance(&coin), 1000);
    }

    #[test]
    fn test_get_coin_balance_short_bytes() {
        let short_bytes = vec![0u8; 10];
        assert_eq!(get_coin_balance(&short_bytes), 0);
    }
}
