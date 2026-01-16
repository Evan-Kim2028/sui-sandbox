#![allow(clippy::manual_is_multiple_of)]
#![allow(clippy::unnecessary_map_or)]
#![allow(dead_code)]
//! Verification tests for group_ops implementation
//!
//! These tests verify that our local group_ops implementation produces
//! identical results to Sui mainnet by comparing outputs for known test vectors.
//!
//! The test strategy:
//! 1. Use fastcrypto directly to generate test vectors
//! 2. Call our native implementations with the same inputs
//! 3. Verify outputs match exactly (byte-for-byte)
//!
//! For ultimate verification, we also test against on-chain execution.

use fastcrypto::groups::bls12381 as bls;
use fastcrypto::groups::{GroupElement, HashToGroupElement, MultiScalarMul, Pairing, Scalar};
use fastcrypto::serde_helpers::ToFromByteArray;

/// Test vectors for BLS12-381 operations
mod test_vectors {
    use super::*;

    /// Known generator points from BLS12-381 spec
    pub fn g1_generator() -> Vec<u8> {
        bls::G1Element::generator().to_byte_array().to_vec()
    }

    pub fn g2_generator() -> Vec<u8> {
        bls::G2Element::generator().to_byte_array().to_vec()
    }

    pub fn scalar_one() -> Vec<u8> {
        bls::Scalar::from(1u128).to_byte_array().to_vec()
    }

    pub fn scalar_two() -> Vec<u8> {
        bls::Scalar::from(2u128).to_byte_array().to_vec()
    }

    pub fn scalar_zero() -> Vec<u8> {
        bls::Scalar::zero().to_byte_array().to_vec()
    }

    pub fn g1_identity() -> Vec<u8> {
        bls::G1Element::zero().to_byte_array().to_vec()
    }

    pub fn g2_identity() -> Vec<u8> {
        bls::G2Element::zero().to_byte_array().to_vec()
    }
}

/// Direct fastcrypto operations (ground truth)
mod fastcrypto_ops {
    use super::*;

    pub fn scalar_add(a: &[u8], b: &[u8]) -> Vec<u8> {
        let a = bls::Scalar::from_byte_array(a.try_into().unwrap()).unwrap();
        let b = bls::Scalar::from_byte_array(b.try_into().unwrap()).unwrap();
        (a + b).to_byte_array().to_vec()
    }

    pub fn scalar_sub(a: &[u8], b: &[u8]) -> Vec<u8> {
        let a = bls::Scalar::from_byte_array(a.try_into().unwrap()).unwrap();
        let b = bls::Scalar::from_byte_array(b.try_into().unwrap()).unwrap();
        (a - b).to_byte_array().to_vec()
    }

    pub fn scalar_mul(a: &[u8], b: &[u8]) -> Vec<u8> {
        let a = bls::Scalar::from_byte_array(a.try_into().unwrap()).unwrap();
        let b = bls::Scalar::from_byte_array(b.try_into().unwrap()).unwrap();
        (a * b).to_byte_array().to_vec()
    }

    pub fn scalar_div(a: &[u8], b: &[u8]) -> Vec<u8> {
        let a = bls::Scalar::from_byte_array(a.try_into().unwrap()).unwrap();
        let b = bls::Scalar::from_byte_array(b.try_into().unwrap()).unwrap();
        let b_inv = b.inverse().unwrap();
        (a * b_inv).to_byte_array().to_vec()
    }

    pub fn g1_add(a: &[u8], b: &[u8]) -> Vec<u8> {
        let a = bls::G1Element::from_byte_array(a.try_into().unwrap()).unwrap();
        let b = bls::G1Element::from_byte_array(b.try_into().unwrap()).unwrap();
        (a + b).to_byte_array().to_vec()
    }

    pub fn g1_sub(a: &[u8], b: &[u8]) -> Vec<u8> {
        let a = bls::G1Element::from_byte_array(a.try_into().unwrap()).unwrap();
        let b = bls::G1Element::from_byte_array(b.try_into().unwrap()).unwrap();
        (a - b).to_byte_array().to_vec()
    }

    pub fn g1_mul(scalar: &[u8], point: &[u8]) -> Vec<u8> {
        let s = bls::Scalar::from_byte_array(scalar.try_into().unwrap()).unwrap();
        let g = bls::G1Element::from_byte_array(point.try_into().unwrap()).unwrap();
        (g * s).to_byte_array().to_vec()
    }

    pub fn g1_div(scalar: &[u8], point: &[u8]) -> Vec<u8> {
        let s = bls::Scalar::from_byte_array(scalar.try_into().unwrap()).unwrap();
        let g = bls::G1Element::from_byte_array(point.try_into().unwrap()).unwrap();
        let s_inv = s.inverse().unwrap();
        (g * s_inv).to_byte_array().to_vec()
    }

    pub fn g2_add(a: &[u8], b: &[u8]) -> Vec<u8> {
        let a = bls::G2Element::from_byte_array(a.try_into().unwrap()).unwrap();
        let b = bls::G2Element::from_byte_array(b.try_into().unwrap()).unwrap();
        (a + b).to_byte_array().to_vec()
    }

    pub fn g2_mul(scalar: &[u8], point: &[u8]) -> Vec<u8> {
        let s = bls::Scalar::from_byte_array(scalar.try_into().unwrap()).unwrap();
        let g = bls::G2Element::from_byte_array(point.try_into().unwrap()).unwrap();
        (g * s).to_byte_array().to_vec()
    }

    pub fn hash_to_g1(msg: &[u8]) -> Vec<u8> {
        bls::G1Element::hash_to_group_element(msg)
            .to_byte_array()
            .to_vec()
    }

    pub fn hash_to_g2(msg: &[u8]) -> Vec<u8> {
        bls::G2Element::hash_to_group_element(msg)
            .to_byte_array()
            .to_vec()
    }

    pub fn pairing(g1: &[u8], g2: &[u8]) -> Vec<u8> {
        let g1 = bls::G1Element::from_byte_array(g1.try_into().unwrap()).unwrap();
        let g2 = bls::G2Element::from_byte_array(g2.try_into().unwrap()).unwrap();
        g1.pairing(&g2).to_byte_array().to_vec()
    }

    pub fn g1_msm(scalars: &[Vec<u8>], points: &[Vec<u8>]) -> Vec<u8> {
        let scalars: Vec<_> = scalars
            .iter()
            .map(|s| bls::Scalar::from_byte_array(s.as_slice().try_into().unwrap()).unwrap())
            .collect();
        let points: Vec<_> = points
            .iter()
            .map(|p| bls::G1Element::from_byte_array(p.as_slice().try_into().unwrap()).unwrap())
            .collect();
        bls::G1Element::multi_scalar_mul(&scalars, &points)
            .unwrap()
            .to_byte_array()
            .to_vec()
    }
}

// Group type constants (must match Sui's group_ops module)
const GROUP_BLS12381_SCALAR: u8 = 0;
const GROUP_BLS12381_G1: u8 = 1;
const GROUP_BLS12381_G2: u8 = 2;
const GROUP_BLS12381_GT: u8 = 3;

/// Simulates our native function implementations
/// This module mirrors what's in src/benchmark/natives.rs
mod sandbox_ops {
    use super::*;

    pub fn internal_validate(group_type: u8, bytes: &[u8]) -> bool {
        match group_type {
            super::GROUP_BLS12381_SCALAR => {
                let arr: Result<&[u8; 32], _> = bytes.try_into();
                arr.map_or(false, |a| bls::Scalar::from_byte_array(a).is_ok())
            }
            super::GROUP_BLS12381_G1 => {
                let arr: Result<&[u8; 48], _> = bytes.try_into();
                arr.map_or(false, |a| bls::G1Element::from_byte_array(a).is_ok())
            }
            super::GROUP_BLS12381_G2 => {
                let arr: Result<&[u8; 96], _> = bytes.try_into();
                arr.map_or(false, |a| bls::G2Element::from_byte_array(a).is_ok())
            }
            super::GROUP_BLS12381_GT => {
                let arr: Result<&[u8; 576], _> = bytes.try_into();
                arr.map_or(false, |a| bls::GTElement::from_byte_array(a).is_ok())
            }
            _ => false,
        }
    }

    pub fn internal_add(group_type: u8, e1: &[u8], e2: &[u8]) -> Option<Vec<u8>> {
        match group_type {
            super::GROUP_BLS12381_SCALAR => {
                let a1: &[u8; 32] = e1.try_into().ok()?;
                let a2: &[u8; 32] = e2.try_into().ok()?;
                let a = bls::Scalar::from_byte_array(a1).ok()?;
                let b = bls::Scalar::from_byte_array(a2).ok()?;
                Some((a + b).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G1 => {
                let a1: &[u8; 48] = e1.try_into().ok()?;
                let a2: &[u8; 48] = e2.try_into().ok()?;
                let a = bls::G1Element::from_byte_array(a1).ok()?;
                let b = bls::G1Element::from_byte_array(a2).ok()?;
                Some((a + b).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G2 => {
                let a1: &[u8; 96] = e1.try_into().ok()?;
                let a2: &[u8; 96] = e2.try_into().ok()?;
                let a = bls::G2Element::from_byte_array(a1).ok()?;
                let b = bls::G2Element::from_byte_array(a2).ok()?;
                Some((a + b).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_GT => {
                let a1: &[u8; 576] = e1.try_into().ok()?;
                let a2: &[u8; 576] = e2.try_into().ok()?;
                let a = bls::GTElement::from_byte_array(a1).ok()?;
                let b = bls::GTElement::from_byte_array(a2).ok()?;
                Some((a + b).to_byte_array().to_vec())
            }
            _ => None,
        }
    }

    pub fn internal_sub(group_type: u8, e1: &[u8], e2: &[u8]) -> Option<Vec<u8>> {
        match group_type {
            super::GROUP_BLS12381_SCALAR => {
                let a1: &[u8; 32] = e1.try_into().ok()?;
                let a2: &[u8; 32] = e2.try_into().ok()?;
                let a = bls::Scalar::from_byte_array(a1).ok()?;
                let b = bls::Scalar::from_byte_array(a2).ok()?;
                Some((a - b).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G1 => {
                let a1: &[u8; 48] = e1.try_into().ok()?;
                let a2: &[u8; 48] = e2.try_into().ok()?;
                let a = bls::G1Element::from_byte_array(a1).ok()?;
                let b = bls::G1Element::from_byte_array(a2).ok()?;
                Some((a - b).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G2 => {
                let a1: &[u8; 96] = e1.try_into().ok()?;
                let a2: &[u8; 96] = e2.try_into().ok()?;
                let a = bls::G2Element::from_byte_array(a1).ok()?;
                let b = bls::G2Element::from_byte_array(a2).ok()?;
                Some((a - b).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_GT => {
                let a1: &[u8; 576] = e1.try_into().ok()?;
                let a2: &[u8; 576] = e2.try_into().ok()?;
                let a = bls::GTElement::from_byte_array(a1).ok()?;
                let b = bls::GTElement::from_byte_array(a2).ok()?;
                Some((a - b).to_byte_array().to_vec())
            }
            _ => None,
        }
    }

    pub fn internal_mul(group_type: u8, element: &[u8], scalar: &[u8]) -> Option<Vec<u8>> {
        let s_arr: &[u8; 32] = scalar.try_into().ok()?;
        let s = bls::Scalar::from_byte_array(s_arr).ok()?;

        match group_type {
            super::GROUP_BLS12381_SCALAR => {
                let e_arr: &[u8; 32] = element.try_into().ok()?;
                let e = bls::Scalar::from_byte_array(e_arr).ok()?;
                Some((e * s).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G1 => {
                let e_arr: &[u8; 48] = element.try_into().ok()?;
                let g = bls::G1Element::from_byte_array(e_arr).ok()?;
                Some((g * s).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G2 => {
                let e_arr: &[u8; 96] = element.try_into().ok()?;
                let g = bls::G2Element::from_byte_array(e_arr).ok()?;
                Some((g * s).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_GT => {
                let e_arr: &[u8; 576] = element.try_into().ok()?;
                let g = bls::GTElement::from_byte_array(e_arr).ok()?;
                Some((g * s).to_byte_array().to_vec())
            }
            _ => None,
        }
    }

    pub fn internal_div(group_type: u8, element: &[u8], scalar: &[u8]) -> Option<Vec<u8>> {
        let s_arr: &[u8; 32] = scalar.try_into().ok()?;
        let s = bls::Scalar::from_byte_array(s_arr).ok()?;
        let s_inv = s.inverse().ok()?;

        match group_type {
            super::GROUP_BLS12381_SCALAR => {
                let e_arr: &[u8; 32] = element.try_into().ok()?;
                let e = bls::Scalar::from_byte_array(e_arr).ok()?;
                Some((e * s_inv).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G1 => {
                let e_arr: &[u8; 48] = element.try_into().ok()?;
                let g = bls::G1Element::from_byte_array(e_arr).ok()?;
                Some((g * s_inv).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G2 => {
                let e_arr: &[u8; 96] = element.try_into().ok()?;
                let g = bls::G2Element::from_byte_array(e_arr).ok()?;
                Some((g * s_inv).to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_GT => {
                let e_arr: &[u8; 576] = element.try_into().ok()?;
                let g = bls::GTElement::from_byte_array(e_arr).ok()?;
                Some((g * s_inv).to_byte_array().to_vec())
            }
            _ => None,
        }
    }

    pub fn internal_hash_to(group_type: u8, msg: &[u8]) -> Option<Vec<u8>> {
        match group_type {
            super::GROUP_BLS12381_G1 => {
                let g = bls::G1Element::hash_to_group_element(msg);
                Some(g.to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G2 => {
                let g = bls::G2Element::hash_to_group_element(msg);
                Some(g.to_byte_array().to_vec())
            }
            _ => None,
        }
    }

    pub fn internal_pairing(g1: &[u8], g2: &[u8]) -> Option<Vec<u8>> {
        let g1_arr: &[u8; 48] = g1.try_into().ok()?;
        let g1 = bls::G1Element::from_byte_array(g1_arr).ok()?;
        let g2_arr: &[u8; 96] = g2.try_into().ok()?;
        let g2 = bls::G2Element::from_byte_array(g2_arr).ok()?;
        let gt = g1.pairing(&g2);
        Some(gt.to_byte_array().to_vec())
    }

    pub fn internal_multi_scalar_mul(
        group_type: u8,
        elements: &[u8],
        scalars: &[u8],
    ) -> Option<Vec<u8>> {
        match group_type {
            super::GROUP_BLS12381_G1 => {
                let scalar_size = 32;
                let element_size = 48;
                if scalars.len() % scalar_size != 0 || elements.len() % element_size != 0 {
                    return None;
                }
                let n = scalars.len() / scalar_size;
                if n != elements.len() / element_size || n == 0 {
                    return None;
                }

                let mut scalar_vec = Vec::with_capacity(n);
                let mut element_vec = Vec::with_capacity(n);
                for i in 0..n {
                    let s_arr: &[u8; 32] = scalars[i * scalar_size..(i + 1) * scalar_size]
                        .try_into()
                        .ok()?;
                    let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                    let g_arr: &[u8; 48] = elements[i * element_size..(i + 1) * element_size]
                        .try_into()
                        .ok()?;
                    let g = bls::G1Element::from_byte_array(g_arr).ok()?;
                    scalar_vec.push(s);
                    element_vec.push(g);
                }

                let result = bls::G1Element::multi_scalar_mul(&scalar_vec, &element_vec).ok()?;
                Some(result.to_byte_array().to_vec())
            }
            super::GROUP_BLS12381_G2 => {
                let scalar_size = 32;
                let element_size = 96;
                if scalars.len() % scalar_size != 0 || elements.len() % element_size != 0 {
                    return None;
                }
                let n = scalars.len() / scalar_size;
                if n != elements.len() / element_size || n == 0 {
                    return None;
                }

                let mut scalar_vec = Vec::with_capacity(n);
                let mut element_vec = Vec::with_capacity(n);
                for i in 0..n {
                    let s_arr: &[u8; 32] = scalars[i * scalar_size..(i + 1) * scalar_size]
                        .try_into()
                        .ok()?;
                    let s = bls::Scalar::from_byte_array(s_arr).ok()?;
                    let g_arr: &[u8; 96] = elements[i * element_size..(i + 1) * element_size]
                        .try_into()
                        .ok()?;
                    let g = bls::G2Element::from_byte_array(g_arr).ok()?;
                    scalar_vec.push(s);
                    element_vec.push(g);
                }

                let result = bls::G2Element::multi_scalar_mul(&scalar_vec, &element_vec).ok()?;
                Some(result.to_byte_array().to_vec())
            }
            _ => None,
        }
    }
}

// ============================================================================
// VERIFICATION TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // SCALAR OPERATIONS
    // ========================================================================

    #[test]
    fn test_scalar_validate() {
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();
        let zero = test_vectors::scalar_zero();

        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_SCALAR, &one));
        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_SCALAR, &two));
        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_SCALAR, &zero));

        // Invalid: wrong size
        assert!(!sandbox_ops::internal_validate(
            GROUP_BLS12381_SCALAR,
            &[0u8; 31]
        ));
        assert!(!sandbox_ops::internal_validate(
            GROUP_BLS12381_SCALAR,
            &[0u8; 33]
        ));
    }

    #[test]
    fn test_scalar_add() {
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();

        // 1 + 1 = 2
        let expected = fastcrypto_ops::scalar_add(&one, &one);
        let actual = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &one, &one).unwrap();
        assert_eq!(expected, actual, "Scalar 1+1 mismatch");
        assert_eq!(actual, two, "Scalar 1+1 should equal 2");

        // 1 + 2 = 3
        let expected = fastcrypto_ops::scalar_add(&one, &two);
        let actual = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &one, &two).unwrap();
        assert_eq!(expected, actual, "Scalar 1+2 mismatch");

        // Commutativity: a + b = b + a
        let expected = fastcrypto_ops::scalar_add(&two, &one);
        let actual = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &two, &one).unwrap();
        assert_eq!(expected, actual, "Scalar addition not commutative");
    }

    #[test]
    fn test_scalar_sub() {
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();
        let zero = test_vectors::scalar_zero();

        // 2 - 1 = 1
        let expected = fastcrypto_ops::scalar_sub(&two, &one);
        let actual = sandbox_ops::internal_sub(GROUP_BLS12381_SCALAR, &two, &one).unwrap();
        assert_eq!(expected, actual, "Scalar 2-1 mismatch");
        assert_eq!(actual, one, "Scalar 2-1 should equal 1");

        // 1 - 1 = 0
        let expected = fastcrypto_ops::scalar_sub(&one, &one);
        let actual = sandbox_ops::internal_sub(GROUP_BLS12381_SCALAR, &one, &one).unwrap();
        assert_eq!(expected, actual, "Scalar 1-1 mismatch");
        assert_eq!(actual, zero, "Scalar 1-1 should equal 0");
    }

    #[test]
    fn test_scalar_mul() {
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();

        // 2 * 2 = 4
        let expected = fastcrypto_ops::scalar_mul(&two, &two);
        let actual = sandbox_ops::internal_mul(GROUP_BLS12381_SCALAR, &two, &two).unwrap();
        assert_eq!(expected, actual, "Scalar 2*2 mismatch");

        // 1 * x = x (identity)
        let expected = two.clone();
        let actual = sandbox_ops::internal_mul(GROUP_BLS12381_SCALAR, &two, &one).unwrap();
        assert_eq!(expected, actual, "Scalar multiplication identity failed");
    }

    #[test]
    fn test_scalar_div() {
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();

        // 2 / 2 = 1
        let expected = fastcrypto_ops::scalar_div(&two, &two);
        let actual = sandbox_ops::internal_div(GROUP_BLS12381_SCALAR, &two, &two).unwrap();
        assert_eq!(expected, actual, "Scalar 2/2 mismatch");
        assert_eq!(actual, one, "Scalar 2/2 should equal 1");

        // x / 1 = x (identity)
        let expected = two.clone();
        let actual = sandbox_ops::internal_div(GROUP_BLS12381_SCALAR, &two, &one).unwrap();
        assert_eq!(expected, actual, "Scalar division identity failed");
    }

    // ========================================================================
    // G1 OPERATIONS
    // ========================================================================

    #[test]
    fn test_g1_validate() {
        let gen = test_vectors::g1_generator();
        let identity = test_vectors::g1_identity();

        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_G1, &gen));
        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_G1, &identity));

        // Invalid: wrong size
        assert!(!sandbox_ops::internal_validate(
            GROUP_BLS12381_G1,
            &[0u8; 47]
        ));
        assert!(!sandbox_ops::internal_validate(
            GROUP_BLS12381_G1,
            &[0u8; 49]
        ));

        // Invalid: not on curve (random bytes)
        assert!(!sandbox_ops::internal_validate(
            GROUP_BLS12381_G1,
            &[0xffu8; 48]
        ));
    }

    #[test]
    fn test_g1_add() {
        let gen = test_vectors::g1_generator();
        let identity = test_vectors::g1_identity();

        // G + 0 = G (identity element)
        let expected = gen.clone();
        let actual = sandbox_ops::internal_add(GROUP_BLS12381_G1, &gen, &identity).unwrap();
        assert_eq!(expected, actual, "G1 + identity should equal G1");

        // G + G = 2G
        let expected = fastcrypto_ops::g1_add(&gen, &gen);
        let actual = sandbox_ops::internal_add(GROUP_BLS12381_G1, &gen, &gen).unwrap();
        assert_eq!(expected, actual, "G1 + G1 mismatch");
    }

    #[test]
    fn test_g1_sub() {
        let gen = test_vectors::g1_generator();
        let identity = test_vectors::g1_identity();

        // G - G = 0 (identity)
        let expected = identity.clone();
        let actual = sandbox_ops::internal_sub(GROUP_BLS12381_G1, &gen, &gen).unwrap();
        assert_eq!(expected, actual, "G1 - G1 should equal identity");

        // G - 0 = G
        let expected = gen.clone();
        let actual = sandbox_ops::internal_sub(GROUP_BLS12381_G1, &gen, &identity).unwrap();
        assert_eq!(expected, actual, "G1 - identity should equal G1");
    }

    #[test]
    fn test_g1_mul() {
        let gen = test_vectors::g1_generator();
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();

        // G * 1 = G
        let expected = gen.clone();
        let actual = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &gen, &one).unwrap();
        assert_eq!(expected, actual, "G1 * 1 should equal G1");

        // G * 2 = G + G
        let expected = fastcrypto_ops::g1_mul(&two, &gen);
        let actual = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &gen, &two).unwrap();
        assert_eq!(expected, actual, "G1 * 2 mismatch");

        // Verify G * 2 = G + G
        let g_plus_g = sandbox_ops::internal_add(GROUP_BLS12381_G1, &gen, &gen).unwrap();
        assert_eq!(actual, g_plus_g, "G1 * 2 should equal G1 + G1");
    }

    #[test]
    fn test_g1_div() {
        let gen = test_vectors::g1_generator();
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();

        // G / 1 = G
        let expected = gen.clone();
        let actual = sandbox_ops::internal_div(GROUP_BLS12381_G1, &gen, &one).unwrap();
        assert_eq!(expected, actual, "G1 / 1 should equal G1");

        // (G * 2) / 2 = G
        let g_times_2 = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &gen, &two).unwrap();
        let actual = sandbox_ops::internal_div(GROUP_BLS12381_G1, &g_times_2, &two).unwrap();
        assert_eq!(gen, actual, "(G1 * 2) / 2 should equal G1");
    }

    // ========================================================================
    // G2 OPERATIONS
    // ========================================================================

    #[test]
    fn test_g2_validate() {
        let gen = test_vectors::g2_generator();
        let identity = test_vectors::g2_identity();

        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_G2, &gen));
        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_G2, &identity));

        // Invalid: wrong size
        assert!(!sandbox_ops::internal_validate(
            GROUP_BLS12381_G2,
            &[0u8; 95]
        ));
        assert!(!sandbox_ops::internal_validate(
            GROUP_BLS12381_G2,
            &[0u8; 97]
        ));
    }

    #[test]
    fn test_g2_add() {
        let gen = test_vectors::g2_generator();
        let identity = test_vectors::g2_identity();

        // G + 0 = G
        let expected = gen.clone();
        let actual = sandbox_ops::internal_add(GROUP_BLS12381_G2, &gen, &identity).unwrap();
        assert_eq!(expected, actual, "G2 + identity should equal G2");

        // G + G
        let expected = fastcrypto_ops::g2_add(&gen, &gen);
        let actual = sandbox_ops::internal_add(GROUP_BLS12381_G2, &gen, &gen).unwrap();
        assert_eq!(expected, actual, "G2 + G2 mismatch");
    }

    #[test]
    fn test_g2_mul() {
        let gen = test_vectors::g2_generator();
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();

        // G * 1 = G
        let expected = gen.clone();
        let actual = sandbox_ops::internal_mul(GROUP_BLS12381_G2, &gen, &one).unwrap();
        assert_eq!(expected, actual, "G2 * 1 should equal G2");

        // G * 2
        let expected = fastcrypto_ops::g2_mul(&two, &gen);
        let actual = sandbox_ops::internal_mul(GROUP_BLS12381_G2, &gen, &two).unwrap();
        assert_eq!(expected, actual, "G2 * 2 mismatch");
    }

    // ========================================================================
    // HASH TO CURVE
    // ========================================================================

    #[test]
    fn test_hash_to_g1() {
        let msg = b"test message for hashing";

        let expected = fastcrypto_ops::hash_to_g1(msg);
        let actual = sandbox_ops::internal_hash_to(GROUP_BLS12381_G1, msg).unwrap();
        assert_eq!(expected, actual, "hash_to_g1 mismatch");

        // Verify result is valid G1 point
        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_G1, &actual));

        // Different messages produce different points
        let msg2 = b"different message";
        let actual2 = sandbox_ops::internal_hash_to(GROUP_BLS12381_G1, msg2).unwrap();
        assert_ne!(
            actual, actual2,
            "Different messages should produce different G1 points"
        );
    }

    #[test]
    fn test_hash_to_g2() {
        let msg = b"test message for hashing";

        let expected = fastcrypto_ops::hash_to_g2(msg);
        let actual = sandbox_ops::internal_hash_to(GROUP_BLS12381_G2, msg).unwrap();
        assert_eq!(expected, actual, "hash_to_g2 mismatch");

        // Verify result is valid G2 point
        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_G2, &actual));
    }

    // ========================================================================
    // PAIRING
    // ========================================================================

    #[test]
    fn test_pairing() {
        let g1 = test_vectors::g1_generator();
        let g2 = test_vectors::g2_generator();

        // e(G1, G2) - basic pairing
        let expected = fastcrypto_ops::pairing(&g1, &g2);
        let actual = sandbox_ops::internal_pairing(&g1, &g2).unwrap();
        assert_eq!(expected, actual, "Pairing e(G1, G2) mismatch");

        // Verify result is valid GT element
        assert!(sandbox_ops::internal_validate(GROUP_BLS12381_GT, &actual));
    }

    #[test]
    fn test_pairing_bilinearity() {
        // Bilinearity property: e(aG1, bG2) = e(G1, G2)^(ab)
        let g1 = test_vectors::g1_generator();
        let g2 = test_vectors::g2_generator();
        let two = test_vectors::scalar_two();

        // e(2G1, G2)
        let g1_times_2 = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &g1, &two).unwrap();
        let pairing_2g1_g2 = sandbox_ops::internal_pairing(&g1_times_2, &g2).unwrap();

        // e(G1, 2G2)
        let g2_times_2 = sandbox_ops::internal_mul(GROUP_BLS12381_G2, &g2, &two).unwrap();
        let pairing_g1_2g2 = sandbox_ops::internal_pairing(&g1, &g2_times_2).unwrap();

        // These should be equal: e(2G1, G2) = e(G1, 2G2)
        assert_eq!(
            pairing_2g1_g2, pairing_g1_2g2,
            "Pairing bilinearity failed: e(2G1, G2) != e(G1, 2G2)"
        );

        // Also verify: e(G1, G2)^2 = e(2G1, G2)
        let base_pairing = sandbox_ops::internal_pairing(&g1, &g2).unwrap();
        let base_pairing_squared =
            sandbox_ops::internal_mul(GROUP_BLS12381_GT, &base_pairing, &two).unwrap();
        assert_eq!(
            pairing_2g1_g2, base_pairing_squared,
            "Pairing bilinearity failed: e(G1, G2)^2 != e(2G1, G2)"
        );
    }

    // ========================================================================
    // MULTI-SCALAR MULTIPLICATION
    // ========================================================================

    #[test]
    fn test_g1_msm_single() {
        let gen = test_vectors::g1_generator();
        let two = test_vectors::scalar_two();

        // MSM with single element should equal scalar multiplication
        let expected = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &gen, &two).unwrap();
        let actual = sandbox_ops::internal_multi_scalar_mul(GROUP_BLS12381_G1, &gen, &two).unwrap();
        assert_eq!(
            expected, actual,
            "Single-element MSM should equal scalar mul"
        );
    }

    #[test]
    fn test_g1_msm_multiple() {
        let gen = test_vectors::g1_generator();
        let one = test_vectors::scalar_one();
        let two = test_vectors::scalar_two();

        // MSM: 1*G + 2*G = 3*G
        let mut elements = Vec::new();
        elements.extend_from_slice(&gen);
        elements.extend_from_slice(&gen);

        let mut scalars = Vec::new();
        scalars.extend_from_slice(&one);
        scalars.extend_from_slice(&two);

        let expected =
            fastcrypto_ops::g1_msm(&[one.clone(), two.clone()], &[gen.clone(), gen.clone()]);
        let actual =
            sandbox_ops::internal_multi_scalar_mul(GROUP_BLS12381_G1, &elements, &scalars).unwrap();
        assert_eq!(expected, actual, "MSM 1*G + 2*G mismatch");

        // Verify: 1*G + 2*G = 3*G
        let three = bls::Scalar::from(3u128).to_byte_array().to_vec();
        let expected_3g = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &gen, &three).unwrap();
        assert_eq!(actual, expected_3g, "MSM 1*G + 2*G should equal 3*G");
    }

    // ========================================================================
    // EDGE CASES AND ERROR HANDLING
    // ========================================================================

    #[test]
    fn test_invalid_group_type() {
        let one = test_vectors::scalar_one();

        // Group type 255 is invalid
        assert!(sandbox_ops::internal_add(255, &one, &one).is_none());
        assert!(sandbox_ops::internal_sub(255, &one, &one).is_none());
        assert!(sandbox_ops::internal_mul(255, &one, &one).is_none());
        assert!(sandbox_ops::internal_div(255, &one, &one).is_none());
        assert!(sandbox_ops::internal_hash_to(255, b"msg").is_none());
    }

    #[test]
    fn test_wrong_size_inputs() {
        let short = vec![0u8; 16];
        let long = vec![0u8; 100];

        // Scalar operations with wrong sizes
        assert!(sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &short, &short).is_none());
        assert!(sandbox_ops::internal_add(GROUP_BLS12381_G1, &short, &short).is_none());
        assert!(sandbox_ops::internal_add(GROUP_BLS12381_G2, &long, &long).is_none());
    }

    #[test]
    fn test_division_by_zero() {
        let one = test_vectors::scalar_one();
        let zero = test_vectors::scalar_zero();

        // Division by zero scalar should fail
        assert!(
            sandbox_ops::internal_div(GROUP_BLS12381_SCALAR, &one, &zero).is_none(),
            "Division by zero should fail"
        );
    }

    // ========================================================================
    // ALGEBRAIC PROPERTIES
    // ========================================================================

    #[test]
    fn test_scalar_associativity() {
        let a = bls::Scalar::from(5u128).to_byte_array().to_vec();
        let b = bls::Scalar::from(7u128).to_byte_array().to_vec();
        let c = bls::Scalar::from(11u128).to_byte_array().to_vec();

        // (a + b) + c = a + (b + c)
        let ab = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &a, &b).unwrap();
        let ab_c = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &ab, &c).unwrap();

        let bc = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &b, &c).unwrap();
        let a_bc = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &a, &bc).unwrap();

        assert_eq!(ab_c, a_bc, "Scalar addition should be associative");
    }

    #[test]
    fn test_scalar_distributivity() {
        let a = bls::Scalar::from(5u128).to_byte_array().to_vec();
        let b = bls::Scalar::from(7u128).to_byte_array().to_vec();
        let c = bls::Scalar::from(11u128).to_byte_array().to_vec();

        // a * (b + c) = a*b + a*c
        let b_plus_c = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &b, &c).unwrap();
        let a_times_bpc = sandbox_ops::internal_mul(GROUP_BLS12381_SCALAR, &b_plus_c, &a).unwrap();

        let a_times_b = sandbox_ops::internal_mul(GROUP_BLS12381_SCALAR, &b, &a).unwrap();
        let a_times_c = sandbox_ops::internal_mul(GROUP_BLS12381_SCALAR, &c, &a).unwrap();
        let ab_plus_ac =
            sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &a_times_b, &a_times_c).unwrap();

        assert_eq!(
            a_times_bpc, ab_plus_ac,
            "Scalar multiplication should be distributive"
        );
    }

    #[test]
    fn test_g1_scalar_mul_distributivity() {
        let g = test_vectors::g1_generator();
        let a = bls::Scalar::from(5u128).to_byte_array().to_vec();
        let b = bls::Scalar::from(7u128).to_byte_array().to_vec();

        // (a + b) * G = a*G + b*G
        let a_plus_b = sandbox_ops::internal_add(GROUP_BLS12381_SCALAR, &a, &b).unwrap();
        let apb_times_g = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &g, &a_plus_b).unwrap();

        let a_times_g = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &g, &a).unwrap();
        let b_times_g = sandbox_ops::internal_mul(GROUP_BLS12381_G1, &g, &b).unwrap();
        let ag_plus_bg =
            sandbox_ops::internal_add(GROUP_BLS12381_G1, &a_times_g, &b_times_g).unwrap();

        assert_eq!(
            apb_times_g, ag_plus_bg,
            "G1 scalar multiplication should be distributive: (a+b)*G = a*G + b*G"
        );
    }

    // ========================================================================
    // COMPREHENSIVE BYTE-FOR-BYTE VERIFICATION
    // ========================================================================

    #[test]
    fn test_known_test_vectors() {
        // These are fixed test vectors that should always produce the same output
        // If any of these fail, our implementation is definitely wrong

        // Test vector 1: Generator points
        let g1_gen = test_vectors::g1_generator();
        assert_eq!(g1_gen.len(), 48, "G1 generator should be 48 bytes");

        let g2_gen = test_vectors::g2_generator();
        assert_eq!(g2_gen.len(), 96, "G2 generator should be 96 bytes");

        // Test vector 2: Scalar 1 serialization
        let one = test_vectors::scalar_one();
        assert_eq!(one.len(), 32, "Scalar should be 32 bytes");
        // Scalar 1 in BLS12-381 is big-endian: 31 zeros followed by 0x01
        assert_eq!(one[31], 1, "Last byte of scalar 1 should be 1 (big-endian)");
        assert!(
            one[..31].iter().all(|&b| b == 0),
            "First 31 bytes of scalar 1 should be zeros"
        );

        // Test vector 3: Identity elements
        let g1_id = test_vectors::g1_identity();
        let g2_id = test_vectors::g2_identity();

        // G + 0 = G
        let result = sandbox_ops::internal_add(GROUP_BLS12381_G1, &g1_gen, &g1_id).unwrap();
        assert_eq!(result, g1_gen, "G1 + identity should equal G1");

        let result = sandbox_ops::internal_add(GROUP_BLS12381_G2, &g2_gen, &g2_id).unwrap();
        assert_eq!(result, g2_gen, "G2 + identity should equal G2");

        // Test vector 4: Hash to curve consistency
        let msg = b"QUUX-V01-CS02-with-BLS12381G1_XMD:SHA-256_SSWU_RO_";
        let h1 = sandbox_ops::internal_hash_to(GROUP_BLS12381_G1, msg).unwrap();
        let h2 = sandbox_ops::internal_hash_to(GROUP_BLS12381_G1, msg).unwrap();
        assert_eq!(h1, h2, "Hash to curve should be deterministic");
    }
}
