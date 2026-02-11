//! Boundary-heavy random value generation for Move function fuzzing.
//!
//! Generates BCS-encoded values for each [`PureType`], with an aggressive
//! boundary distribution: ~40% exact boundaries, ~30% near-boundary, ~30% uniform.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::classifier::PureType;

/// Random value generator with boundary-heavy distribution.
pub struct ValueGenerator {
    rng: StdRng,
    max_vector_len: usize,
}

impl ValueGenerator {
    /// Create a new generator with the given seed and max vector length.
    pub fn new(seed: u64, max_vector_len: usize) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            max_vector_len,
        }
    }

    /// Generate a random BCS-encoded value for the given pure type.
    pub fn generate(&mut self, ty: &PureType) -> Vec<u8> {
        match ty {
            PureType::Bool => self.gen_bool(),
            PureType::U8 => self.gen_u8(),
            PureType::U16 => self.gen_u16(),
            PureType::U32 => self.gen_u32(),
            PureType::U64 => self.gen_u64(),
            PureType::U128 => self.gen_u128(),
            PureType::U256 => self.gen_u256(),
            PureType::Address => self.gen_address(),
            PureType::VectorBool => self.gen_vector(PureType::Bool),
            PureType::VectorU8 => self.gen_vector(PureType::U8),
            PureType::VectorU16 => self.gen_vector(PureType::U16),
            PureType::VectorU32 => self.gen_vector(PureType::U32),
            PureType::VectorU64 => self.gen_vector(PureType::U64),
            PureType::VectorU128 => self.gen_vector(PureType::U128),
            PureType::VectorU256 => self.gen_vector(PureType::U256),
            PureType::VectorAddress => self.gen_vector(PureType::Address),
            PureType::String => self.gen_string(),
            PureType::AsciiString => self.gen_ascii_string(),
        }
    }

    /// Format a BCS-encoded value as a human-readable string for reporting.
    pub fn format_value(ty: &PureType, bcs_bytes: &[u8]) -> String {
        match ty {
            PureType::Bool => bcs::from_bytes::<bool>(bcs_bytes)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| hex::encode(bcs_bytes)),
            PureType::U8 => bcs::from_bytes::<u8>(bcs_bytes)
                .map(|v| format!("u8:{v}"))
                .unwrap_or_else(|_| hex::encode(bcs_bytes)),
            PureType::U16 => bcs::from_bytes::<u16>(bcs_bytes)
                .map(|v| format!("u16:{v}"))
                .unwrap_or_else(|_| hex::encode(bcs_bytes)),
            PureType::U32 => bcs::from_bytes::<u32>(bcs_bytes)
                .map(|v| format!("u32:{v}"))
                .unwrap_or_else(|_| hex::encode(bcs_bytes)),
            PureType::U64 => bcs::from_bytes::<u64>(bcs_bytes)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| hex::encode(bcs_bytes)),
            PureType::U128 => bcs::from_bytes::<u128>(bcs_bytes)
                .map(|v| format!("u128:{v}"))
                .unwrap_or_else(|_| hex::encode(bcs_bytes)),
            PureType::U256 => format!("u256:0x{}", hex::encode(bcs_bytes)),
            PureType::Address => format!("0x{}", hex::encode(bcs_bytes)),
            PureType::String | PureType::AsciiString => bcs::from_bytes::<Vec<u8>>(bcs_bytes)
                .map(|v| {
                    std::string::String::from_utf8(v)
                        .unwrap_or_else(|e| format!("bytes:{}", hex::encode(e.as_bytes())))
                })
                .unwrap_or_else(|_| hex::encode(bcs_bytes)),
            PureType::VectorBool
            | PureType::VectorU8
            | PureType::VectorU16
            | PureType::VectorU32
            | PureType::VectorU64
            | PureType::VectorU128
            | PureType::VectorU256
            | PureType::VectorAddress => format!("vector:0x{}", hex::encode(bcs_bytes)),
        }
    }

    // ---- Primitive generators ----

    fn gen_bool(&mut self) -> Vec<u8> {
        bcs::to_bytes(&self.rng.gen_bool(0.5)).unwrap()
    }

    fn gen_u8(&mut self) -> Vec<u8> {
        let val = self.gen_integer_u64(&U8_BOUNDARIES, u8::MAX as u64) as u8;
        bcs::to_bytes(&val).unwrap()
    }

    fn gen_u16(&mut self) -> Vec<u8> {
        let val = self.gen_integer_u64(&U16_BOUNDARIES, u16::MAX as u64) as u16;
        bcs::to_bytes(&val).unwrap()
    }

    fn gen_u32(&mut self) -> Vec<u8> {
        let val = self.gen_integer_u64(&U32_BOUNDARIES, u32::MAX as u64) as u32;
        bcs::to_bytes(&val).unwrap()
    }

    fn gen_u64(&mut self) -> Vec<u8> {
        let val = self.gen_integer_u64(&U64_BOUNDARIES, u64::MAX);
        bcs::to_bytes(&val).unwrap()
    }

    fn gen_u128(&mut self) -> Vec<u8> {
        let val = self.gen_integer_u128(&U128_BOUNDARIES, u128::MAX);
        bcs::to_bytes(&val).unwrap()
    }

    fn gen_u256(&mut self) -> Vec<u8> {
        // U256 is 32 bytes in BCS, little-endian
        let tier: f64 = self.rng.gen();
        if tier < 0.4 {
            // Boundary: all zeros, all ones, or power-of-2 patterns
            let choice = self.rng.gen_range(0..4);
            match choice {
                0 => vec![0u8; 32], // 0
                1 => {
                    // 1
                    let mut bytes = vec![0u8; 32];
                    bytes[0] = 1;
                    bytes
                }
                2 => vec![0xFF; 32], // MAX
                _ => {
                    // Random power of 2
                    let mut bytes = vec![0u8; 32];
                    let byte_idx = self.rng.gen_range(0..32);
                    let bit_idx = self.rng.gen_range(0..8);
                    bytes[byte_idx] = 1 << bit_idx;
                    bytes
                }
            }
        } else {
            // Uniform random 32 bytes
            let mut bytes = vec![0u8; 32];
            self.rng.fill(&mut bytes[..]);
            bytes
        }
    }

    fn gen_address(&mut self) -> Vec<u8> {
        let tier: f64 = self.rng.gen();
        if tier < 0.3 {
            // Well-known addresses
            let choice = self.rng.gen_range(0..5);
            let mut addr = [0u8; 32];
            match choice {
                0 => {}               // 0x0
                1 => addr[31] = 1,    // 0x1
                2 => addr[31] = 2,    // 0x2
                3 => addr[31] = 3,    // 0x3
                _ => addr[31] = 0xEE, // sender-like
            }
            addr.to_vec()
        } else {
            // Random 32 bytes
            let mut addr = [0u8; 32];
            self.rng.fill(&mut addr[..]);
            addr.to_vec()
        }
    }

    fn gen_string(&mut self) -> Vec<u8> {
        let len = self.gen_vector_len();
        let s: String = (0..len)
            .map(|_| self.rng.gen_range(0x20..=0x7E) as u8 as char)
            .collect();
        bcs::to_bytes(&s.into_bytes()).unwrap()
    }

    fn gen_ascii_string(&mut self) -> Vec<u8> {
        let len = self.gen_vector_len();
        let bytes: Vec<u8> = (0..len).map(|_| self.rng.gen_range(0x20..=0x7E)).collect();
        bcs::to_bytes(&bytes).unwrap()
    }

    // ---- Vector generator ----

    fn gen_vector(&mut self, element_type: PureType) -> Vec<u8> {
        let len = self.gen_vector_len();
        // BCS vector: ULEB128 length prefix + concatenated element bytes
        let elements: Vec<Vec<u8>> = (0..len).map(|_| self.generate(&element_type)).collect();
        // Use BCS encoding: length as ULEB128 + raw element bytes
        let mut result = Vec::new();
        // ULEB128 encode the length
        let mut val = len;
        loop {
            let mut byte = (val & 0x7F) as u8;
            val >>= 7;
            if val != 0 {
                byte |= 0x80;
            }
            result.push(byte);
            if val == 0 {
                break;
            }
        }
        for elem in &elements {
            result.extend_from_slice(elem);
        }
        result
    }

    /// Generate a vector length with edge-case weighting.
    fn gen_vector_len(&mut self) -> usize {
        let tier: f64 = self.rng.gen();
        if tier < 0.20 {
            0 // empty
        } else if tier < 0.35 {
            1 // single element
        } else if tier < 0.50 {
            self.max_vector_len // max length
        } else {
            self.rng.gen_range(2..=self.max_vector_len)
        }
    }

    // ---- Integer generation with tiered distribution ----

    /// Generate a u64 value with boundary-heavy distribution.
    fn gen_integer_u64(&mut self, boundaries: &[u64], max: u64) -> u64 {
        let tier: f64 = self.rng.gen();
        if tier < 0.4 {
            // Exact boundary value
            boundaries[self.rng.gen_range(0..boundaries.len())]
        } else if tier < 0.7 {
            // Near-boundary: pick a boundary, offset by ±1..16
            let base = boundaries[self.rng.gen_range(0..boundaries.len())];
            let offset = self.rng.gen_range(1..=16_i64);
            if self.rng.gen_bool(0.5) {
                base.saturating_add(offset as u64).min(max)
            } else {
                base.saturating_sub(offset as u64)
            }
        } else {
            // Uniform random
            self.rng.gen_range(0..=max)
        }
    }

    /// Generate a u128 value with boundary-heavy distribution.
    fn gen_integer_u128(&mut self, boundaries: &[u128], max: u128) -> u128 {
        let tier: f64 = self.rng.gen();
        if tier < 0.4 {
            boundaries[self.rng.gen_range(0..boundaries.len())]
        } else if tier < 0.7 {
            let base = boundaries[self.rng.gen_range(0..boundaries.len())];
            let offset = self.rng.gen_range(1..=16_u128);
            if self.rng.gen_bool(0.5) {
                base.saturating_add(offset).min(max)
            } else {
                base.saturating_sub(offset)
            }
        } else {
            // For uniform u128, generate two u64s
            let hi = self.rng.gen::<u64>() as u128;
            let lo = self.rng.gen::<u64>() as u128;
            (hi << 64) | lo
        }
    }
}

// ---- Boundary value tables ----

const U8_BOUNDARIES: [u64; 7] = [0, 1, 2, 127, 128, 254, 255];

const U16_BOUNDARIES: [u64; 9] = [0, 1, 2, 255, 256, 32767, 32768, 65534, 65535];

const U32_BOUNDARIES: [u64; 11] = [
    0,
    1,
    2,
    255,
    256,
    65535,
    65536,
    2_147_483_647, // 2^31 - 1
    2_147_483_648, // 2^31
    4_294_967_294, // 2^32 - 2
    4_294_967_295, // 2^32 - 1
];

const U64_BOUNDARIES: [u64; 15] = [
    0,
    1,
    2,
    255,
    256,
    65535,
    65536,
    2_147_483_647,
    2_147_483_648,
    4_294_967_295,
    4_294_967_296,
    9_223_372_036_854_775_807,  // 2^63 - 1
    9_223_372_036_854_775_808,  // 2^63
    18_446_744_073_709_551_614, // 2^64 - 2
    18_446_744_073_709_551_615, // 2^64 - 1
];

const U128_BOUNDARIES: [u128; 19] = [
    0,
    1,
    2,
    255,
    256,
    65535,
    65536,
    u32::MAX as u128,
    (u32::MAX as u128) + 1,
    u64::MAX as u128,
    (u64::MAX as u128) + 1,
    i64::MAX as u128,
    (i64::MAX as u128) + 1,
    170_141_183_460_469_231_731_687_303_715_884_105_727, // 2^127 - 1
    170_141_183_460_469_231_731_687_303_715_884_105_728, // 2^127
    340_282_366_920_938_463_463_374_607_431_768_211_453, // 2^128 - 3
    340_282_366_920_938_463_463_374_607_431_768_211_454, // 2^128 - 2
    340_282_366_920_938_463_463_374_607_431_768_211_455, // 2^128 - 1 (MAX)
    0, // duplicate 0 for array size alignment — filtered by random selection
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_seed() {
        let mut gen1 = ValueGenerator::new(42, 32);
        let mut gen2 = ValueGenerator::new(42, 32);

        for ty in &[
            PureType::Bool,
            PureType::U8,
            PureType::U64,
            PureType::Address,
        ] {
            let v1 = gen1.generate(ty);
            let v2 = gen2.generate(ty);
            assert_eq!(v1, v2, "Same seed should produce same value for {:?}", ty);
        }
    }

    #[test]
    fn test_u8_bcs_roundtrip() {
        let mut gen = ValueGenerator::new(123, 32);
        for _ in 0..100 {
            let bytes = gen.generate(&PureType::U8);
            let val: u8 = bcs::from_bytes(&bytes).expect("u8 BCS roundtrip");
            assert!(val <= u8::MAX);
        }
    }

    #[test]
    fn test_u64_bcs_roundtrip() {
        let mut gen = ValueGenerator::new(456, 32);
        for _ in 0..100 {
            let bytes = gen.generate(&PureType::U64);
            let _val: u64 = bcs::from_bytes(&bytes).expect("u64 BCS roundtrip");
        }
    }

    #[test]
    fn test_u128_bcs_roundtrip() {
        let mut gen = ValueGenerator::new(789, 32);
        for _ in 0..100 {
            let bytes = gen.generate(&PureType::U128);
            let _val: u128 = bcs::from_bytes(&bytes).expect("u128 BCS roundtrip");
        }
    }

    #[test]
    fn test_bool_bcs_roundtrip() {
        let mut gen = ValueGenerator::new(101, 32);
        for _ in 0..100 {
            let bytes = gen.generate(&PureType::Bool);
            let _val: bool = bcs::from_bytes(&bytes).expect("bool BCS roundtrip");
        }
    }

    #[test]
    fn test_address_is_32_bytes() {
        let mut gen = ValueGenerator::new(202, 32);
        for _ in 0..100 {
            let bytes = gen.generate(&PureType::Address);
            assert_eq!(bytes.len(), 32, "Address should always be 32 bytes");
        }
    }

    #[test]
    fn test_vector_u8_bcs_roundtrip() {
        let mut gen = ValueGenerator::new(303, 16);
        for _ in 0..50 {
            let bytes = gen.generate(&PureType::VectorU8);
            let _val: Vec<u8> = bcs::from_bytes(&bytes).expect("vector<u8> BCS roundtrip");
        }
    }

    #[test]
    fn test_string_bcs_roundtrip() {
        let mut gen = ValueGenerator::new(404, 32);
        for _ in 0..50 {
            let bytes = gen.generate(&PureType::String);
            let val: Vec<u8> = bcs::from_bytes(&bytes).expect("String BCS roundtrip");
            // All bytes should be printable ASCII
            for b in &val {
                assert!(
                    (0x20..=0x7E).contains(b),
                    "String byte {:#x} not in printable ASCII range",
                    b
                );
            }
        }
    }

    #[test]
    fn test_boundaries_appear_in_u8() {
        let mut gen = ValueGenerator::new(505, 32);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let bytes = gen.generate(&PureType::U8);
            let val: u8 = bcs::from_bytes(&bytes).unwrap();
            seen.insert(val);
        }
        // With 1000 iterations and 40% boundary rate, we should see the key boundaries
        assert!(seen.contains(&0), "Should see boundary 0");
        assert!(seen.contains(&255), "Should see boundary 255");
        assert!(seen.contains(&1), "Should see boundary 1");
    }

    #[test]
    fn test_format_value() {
        assert_eq!(
            ValueGenerator::format_value(&PureType::Bool, &bcs::to_bytes(&true).unwrap()),
            "true"
        );
        assert_eq!(
            ValueGenerator::format_value(&PureType::U8, &bcs::to_bytes(&42u8).unwrap()),
            "u8:42"
        );
        assert_eq!(
            ValueGenerator::format_value(&PureType::U64, &bcs::to_bytes(&1000u64).unwrap()),
            "1000"
        );
    }
}
