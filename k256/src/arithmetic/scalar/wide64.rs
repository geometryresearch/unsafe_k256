//! Wide scalar (64-bit limbs)

use super::{Scalar, MODULUS};
use crate::ORDER;
use core::convert::TryInto;
use elliptic_curve::{
    bigint::{Limb, U256},
    subtle::{Choice, ConditionallySelectable},
};

/// Limbs of 2^256 minus the secp256k1 order.
const NEG_MODULUS: [u64; 4] = [!MODULUS[0] + 1, !MODULUS[1], !MODULUS[2], !MODULUS[3]];

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WideScalar(pub(super) [u64; 8]);

impl WideScalar {
    pub fn from_bytes(bytes: &[u8; 64]) -> Self {
        let mut w = [0u64; 8];
        for i in 0..8 {
            w[i] = u64::from_be_bytes(bytes[((7 - i) * 8)..((7 - i) * 8 + 8)].try_into().unwrap());
        }
        Self(w)
    }

    /// Multiplies two scalars without modulo reduction, producing up to a 512-bit scalar.
    #[inline(always)] // only used in Scalar::mul(), so won't cause binary bloat
    pub fn mul_wide(a: &Scalar, b: &Scalar) -> Self {
        let a = a.0.to_uint_array();
        let b = b.0.to_uint_array();

        /* 160 bit accumulator. */
        let c0 = 0;
        let c1 = 0;
        let c2 = 0;

        /* l[0..7] = a[0..3] * b[0..3]. */
        let (c0, c1) = muladd_fast(a[0], b[0], c0, c1);
        let (l0, c0, c1) = (c0, c1, 0);
        let (c0, c1, c2) = muladd(a[0], b[1], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[1], b[0], c0, c1, c2);
        let (l1, c0, c1, c2) = (c0, c1, c2, 0);
        let (c0, c1, c2) = muladd(a[0], b[2], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[1], b[1], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[2], b[0], c0, c1, c2);
        let (l2, c0, c1, c2) = (c0, c1, c2, 0);
        let (c0, c1, c2) = muladd(a[0], b[3], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[1], b[2], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[2], b[1], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[3], b[0], c0, c1, c2);
        let (l3, c0, c1, c2) = (c0, c1, c2, 0);
        let (c0, c1, c2) = muladd(a[1], b[3], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[2], b[2], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[3], b[1], c0, c1, c2);
        let (l4, c0, c1, c2) = (c0, c1, c2, 0);
        let (c0, c1, c2) = muladd(a[2], b[3], c0, c1, c2);
        let (c0, c1, c2) = muladd(a[3], b[2], c0, c1, c2);
        let (l5, c0, c1, _c2) = (c0, c1, c2, 0);
        let (c0, c1) = muladd_fast(a[3], b[3], c0, c1);
        let (l6, c0, _c1) = (c0, c1, 0);
        let l7 = c0;

        Self([l0, l1, l2, l3, l4, l5, l6, l7])
    }

    /// Multiplies `a` by `b` (without modulo reduction) divide the result by `2^shift`
    /// (rounding to the nearest integer).
    /// Variable time in `shift`.
    pub(crate) fn mul_shift_vartime(a: &Scalar, b: &Scalar, shift: usize) -> Scalar {
        debug_assert!(shift >= 256);

        fn ifelse(c: bool, x: u64, y: u64) -> u64 {
            if c {
                x
            } else {
                y
            }
        }

        let l = Self::mul_wide(a, b);
        let shiftlimbs = shift >> 6;
        let shiftlow = shift & 0x3F;
        let shifthigh = 64 - shiftlow;
        let r0 = ifelse(
            shift < 512,
            (l.0[shiftlimbs] >> shiftlow)
                | ifelse(
                    shift < 448 && shiftlow != 0,
                    l.0[1 + shiftlimbs] << shifthigh,
                    0,
                ),
            0,
        );

        let r1 = ifelse(
            shift < 448,
            (l.0[1 + shiftlimbs] >> shiftlow)
                | ifelse(
                    shift < 448 && shiftlow != 0,
                    l.0[2 + shiftlimbs] << shifthigh,
                    0,
                ),
            0,
        );

        let r2 = ifelse(
            shift < 384,
            (l.0[2 + shiftlimbs] >> shiftlow)
                | ifelse(
                    shift < 320 && shiftlow != 0,
                    l.0[3 + shiftlimbs] << shifthigh,
                    0,
                ),
            0,
        );

        let r3 = ifelse(shift < 320, l.0[3 + shiftlimbs] >> shiftlow, 0);

        let res = Scalar(U256::from_uint_array([r0, r1, r2, r3]));

        // Check the highmost discarded bit and round up if it is set.
        let c = (l.0[(shift - 1) >> 6] >> ((shift - 1) & 0x3f)) & 1;
        res.conditional_add_bit(0, Choice::from(c as u8))
    }

    #[inline(always)] // only used in Scalar::mul(), so won't cause binary bloat
    pub(super) fn reduce(&self) -> Scalar {
        let n0 = self.0[4];
        let n1 = self.0[5];
        let n2 = self.0[6];
        let n3 = self.0[7];

        /* Reduce 512 bits into 385. */
        /* m[0..6] = self[0..3] + n[0..3] * NEG_MODULUS. */
        let c0 = self.0[0];
        let c1 = 0;
        let c2 = 0;
        let (c0, c1) = muladd_fast(n0, NEG_MODULUS[0], c0, c1);
        let (m0, c0, c1) = (c0, c1, 0);
        let (c0, c1) = sumadd_fast(self.0[1], c0, c1);
        let (c0, c1, c2) = muladd(n1, NEG_MODULUS[0], c0, c1, c2);
        let (c0, c1, c2) = muladd(n0, NEG_MODULUS[1], c0, c1, c2);
        let (m1, c0, c1, c2) = (c0, c1, c2, 0);
        let (c0, c1, c2) = sumadd(self.0[2], c0, c1, c2);
        let (c0, c1, c2) = muladd(n2, NEG_MODULUS[0], c0, c1, c2);
        let (c0, c1, c2) = muladd(n1, NEG_MODULUS[1], c0, c1, c2);
        let (c0, c1, c2) = sumadd(n0, c0, c1, c2);
        let (m2, c0, c1, c2) = (c0, c1, c2, 0);
        let (c0, c1, c2) = sumadd(self.0[3], c0, c1, c2);
        let (c0, c1, c2) = muladd(n3, NEG_MODULUS[0], c0, c1, c2);
        let (c0, c1, c2) = muladd(n2, NEG_MODULUS[1], c0, c1, c2);
        let (c0, c1, c2) = sumadd(n1, c0, c1, c2);
        let (m3, c0, c1, c2) = (c0, c1, c2, 0);
        let (c0, c1, c2) = muladd(n3, NEG_MODULUS[1], c0, c1, c2);
        let (c0, c1, c2) = sumadd(n2, c0, c1, c2);
        let (m4, c0, c1, _c2) = (c0, c1, c2, 0);
        let (c0, c1) = sumadd_fast(n3, c0, c1);
        let (m5, c0, _c1) = (c0, c1, 0);
        debug_assert!(c0 <= 1);
        let m6 = c0;

        /* Reduce 385 bits into 258. */
        /* p[0..4] = m[0..3] + m[4..6] * NEG_MODULUS. */
        let c0 = m0;
        let c1 = 0;
        let c2 = 0;
        let (c0, c1) = muladd_fast(m4, NEG_MODULUS[0], c0, c1);
        let (p0, c0, c1) = (c0, c1, 0);
        let (c0, c1) = sumadd_fast(m1, c0, c1);
        let (c0, c1, c2) = muladd(m5, NEG_MODULUS[0], c0, c1, c2);
        let (c0, c1, c2) = muladd(m4, NEG_MODULUS[1], c0, c1, c2);
        let (p1, c0, c1) = (c0, c1, 0);
        let (c0, c1, c2) = sumadd(m2, c0, c1, c2);
        let (c0, c1, c2) = muladd(m6, NEG_MODULUS[0], c0, c1, c2);
        let (c0, c1, c2) = muladd(m5, NEG_MODULUS[1], c0, c1, c2);
        let (c0, c1, c2) = sumadd(m4, c0, c1, c2);
        let (p2, c0, c1, _c2) = (c0, c1, c2, 0);
        let (c0, c1) = sumadd_fast(m3, c0, c1);
        let (c0, c1) = muladd_fast(m6, NEG_MODULUS[1], c0, c1);
        let (c0, c1) = sumadd_fast(m5, c0, c1);
        let (p3, c0, _c1) = (c0, c1, 0);
        let p4 = c0 + m6;
        debug_assert!(p4 <= 2);

        /* Reduce 258 bits into 256. */
        /* r[0..3] = p[0..3] + p[4] * NEG_MODULUS. */
        let mut c = (p0 as u128) + (NEG_MODULUS[0] as u128) * (p4 as u128);
        let r0 = (c & 0xFFFFFFFFFFFFFFFFu128) as u64;
        c >>= 64;
        c += (p1 as u128) + (NEG_MODULUS[1] as u128) * (p4 as u128);
        let r1 = (c & 0xFFFFFFFFFFFFFFFFu128) as u64;
        c >>= 64;
        c += (p2 as u128) + (p4 as u128);
        let r2 = (c & 0xFFFFFFFFFFFFFFFFu128) as u64;
        c >>= 64;
        c += p3 as u128;
        let r3 = (c & 0xFFFFFFFFFFFFFFFFu128) as u64;
        c >>= 64;

        /* Final reduction of r. */
        let r = U256::from([r0, r1, r2, r3]);
        let (r2, underflow) = r.sbb(&ORDER, Limb::ZERO);
        let high_bit = Choice::from(c as u8);
        let underflow = Choice::from((underflow.0 >> 63) as u8);
        Scalar(U256::conditional_select(&r, &r2, !underflow | high_bit))
    }
}

/// Constant-time comparison.
#[inline(always)]
fn ct_less(a: u64, b: u64) -> u64 {
    // Do not convert to Choice since it is only used internally,
    // and we don't want loss of performance.
    (a < b) as u64
}

/// Add a to the number defined by (c0,c1,c2). c2 must never overflow.
fn sumadd(a: u64, c0: u64, c1: u64, c2: u64) -> (u64, u64, u64) {
    let new_c0 = c0.wrapping_add(a); // overflow is handled on the next line
    let over = ct_less(new_c0, a);
    let new_c1 = c1.wrapping_add(over); // overflow is handled on the next line
    let new_c2 = c2 + ct_less(new_c1, over); // never overflows by contract
    (new_c0, new_c1, new_c2)
}

/// Add a to the number defined by (c0,c1). c1 must never overflow, c2 must be zero.
fn sumadd_fast(a: u64, c0: u64, c1: u64) -> (u64, u64) {
    let new_c0 = c0.wrapping_add(a); // overflow is handled on the next line
    let new_c1 = c1 + ct_less(new_c0, a); // never overflows by contract (verified the next line)
    debug_assert!((new_c1 != 0) | (new_c0 >= a));
    (new_c0, new_c1)
}

/// Add a*b to the number defined by (c0,c1,c2). c2 must never overflow.
fn muladd(a: u64, b: u64, c0: u64, c1: u64, c2: u64) -> (u64, u64, u64) {
    let t = (a as u128) * (b as u128);
    let th = (t >> 64) as u64; // at most 0xFFFFFFFFFFFFFFFE
    let tl = t as u64;

    let new_c0 = c0.wrapping_add(tl); // overflow is handled on the next line
    let new_th = th + if new_c0 < tl { 1 } else { 0 }; // at most 0xFFFFFFFFFFFFFFFF
    let new_c1 = c1.wrapping_add(new_th); // overflow is handled on the next line
    let new_c2 = c2 + ct_less(new_c1, new_th); // never overflows by contract (verified in the next line)
    debug_assert!((new_c1 >= new_th) || (new_c2 != 0));
    (new_c0, new_c1, new_c2)
}

/// Add a*b to the number defined by (c0,c1). c1 must never overflow.
fn muladd_fast(a: u64, b: u64, c0: u64, c1: u64) -> (u64, u64) {
    let t = (a as u128) * (b as u128);
    let th = (t >> 64) as u64; // at most 0xFFFFFFFFFFFFFFFE
    let tl = t as u64;

    let new_c0 = c0.wrapping_add(tl); // overflow is handled on the next line
    let new_th = th + ct_less(new_c0, tl); // at most 0xFFFFFFFFFFFFFFFF
    let new_c1 = c1 + new_th; // never overflows by contract (verified in the next line)
    debug_assert!(new_c1 >= new_th);
    (new_c0, new_c1)
}