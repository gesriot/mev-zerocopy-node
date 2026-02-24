/// Zero-copy validation layer using the `zerocopy` crate.
///
/// While `bytemuck` is used for the hot path (simple POD cast), `zerocopy`
/// provides a safer, derive-macro-driven API that additionally validates
/// field invariants at cast time. This module shows the complementary usage:
/// - `bytemuck` for maximum throughput in the hot loop (one pointer cast, no checks)
/// - `zerocopy` for the outer validation layer (field range checks, endianness markers)
use zerocopy::{AsBytes, FromBytes, FromZeroes};

/// A validated pool state update broadcast from on-chain relayers.
///
/// `FromBytes` + `AsBytes` from `zerocopy` guarantee that:
/// 1. The type has no padding bytes (safe to reinterpret from wire data).
/// 2. Any bit pattern is a valid instance (no enum discriminant traps).
///
/// This mirrors how Solana program accounts are deserialized in
/// high-throughput indexers (OpenBook, Phoenix) — via `bytemuck` / `zerocopy`
/// rather than Anchor's serde-style `AccountDeserialize`.
#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, AsBytes, FromZeroes)]
pub struct PoolStateUpdate {
    /// Pool address (20 bytes, Ethereum-style or Solana truncated).
    pub pool_address: [u8; 20],
    /// Current reserve of token0 (little-endian u64).
    pub reserve0_le: [u8; 8],
    /// Current reserve of token1 (little-endian u64).
    pub reserve1_le: [u8; 8],
    /// Block/slot number of this update (little-endian u64).
    pub slot_le: [u8; 8],
    /// Sequence number for detecting missed updates (little-endian u32).
    pub seq_le: [u8; 4],
    /// Padding to reach 64-byte cache-line alignment.
    pub _pad: [u8; 16],
}

// Total: 20 + 8 + 8 + 8 + 4 + 16 = 64 bytes — exactly one cache line.
const _: () = assert!(core::mem::size_of::<PoolStateUpdate>() == 64);

impl PoolStateUpdate {
    pub const WIRE_SIZE: usize = core::mem::size_of::<PoolStateUpdate>();

    #[inline(always)]
    pub fn reserve0(&self) -> u64 {
        u64::from_le_bytes(self.reserve0_le)
    }

    #[inline(always)]
    pub fn reserve1(&self) -> u64 {
        u64::from_le_bytes(self.reserve1_le)
    }

    #[inline(always)]
    pub fn slot(&self) -> u64 {
        u64::from_le_bytes(self.slot_le)
    }

    #[inline(always)]
    pub fn seq(&self) -> u32 {
        u32::from_le_bytes(self.seq_le)
    }
}

/// Errors that can occur during pool state validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    /// Slice too short to contain a full `PoolStateUpdate`.
    TooShort,
    /// `zerocopy` layout check failed (misaligned or wrong size).
    LayoutMismatch,
    /// Both reserves are zero — indicates an uninitialized or invalid pool.
    ZeroReserves,
    /// Sequence number gap detected (missed update).
    SequenceGap { expected: u32, got: u32 },
}

/// Validate and zero-copy cast a raw byte slice to a `PoolStateUpdate`.
///
/// Uses `zerocopy::FromBytes::ref_from` — this is a guaranteed-safe
/// pointer cast that also checks alignment and size at runtime.
/// No copy, no allocation.
///
/// Returns `Err(ValidationError)` if the slice is malformed or the pool
/// state fails sanity checks.
#[inline(always)]
pub fn validate_pool_update<'a>(
    data: &'a [u8],
    last_seq: u32,
) -> Result<&'a PoolStateUpdate, ValidationError> {
    if data.len() < PoolStateUpdate::WIRE_SIZE {
        return Err(ValidationError::TooShort);
    }
    // zerocopy::FromBytes::ref_from: zero-copy cast with layout validation.
    let update = PoolStateUpdate::ref_from(&data[..PoolStateUpdate::WIRE_SIZE])
        .ok_or(ValidationError::LayoutMismatch)?;

    if update.reserve0() == 0 && update.reserve1() == 0 {
        return Err(ValidationError::ZeroReserves);
    }

    // Sequence continuity check (wrapping arithmetic for rollover safety)
    let expected = last_seq.wrapping_add(1);
    if update.seq() != expected && last_seq != 0 {
        return Err(ValidationError::SequenceGap {
            expected,
            got: update.seq(),
        });
    }

    Ok(update)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::AsBytes;

    fn make_update(reserve0: u64, reserve1: u64, slot: u64, seq: u32) -> [u8; 64] {
        let update = PoolStateUpdate {
            pool_address: [0xCA; 20],
            reserve0_le: reserve0.to_le_bytes(),
            reserve1_le: reserve1.to_le_bytes(),
            slot_le: slot.to_le_bytes(),
            seq_le: seq.to_le_bytes(),
            _pad: [0u8; 16],
        };
        let mut buf = [0u8; 64];
        buf.copy_from_slice(update.as_bytes());
        buf
    }

    #[test]
    fn zerocopy_cast_reads_fields_correctly() {
        let buf = make_update(1_000_000, 500_000, 9_876_543, 1);
        let update = validate_pool_update(&buf, 0).expect("valid update");
        assert_eq!(update.reserve0(), 1_000_000);
        assert_eq!(update.reserve1(), 500_000);
        assert_eq!(update.slot(), 9_876_543);
        assert_eq!(update.seq(), 1);
    }

    #[test]
    fn zerocopy_rejects_zero_reserves() {
        let buf = make_update(0, 0, 1, 1);
        assert_eq!(validate_pool_update(&buf, 0), Err(ValidationError::ZeroReserves));
    }

    #[test]
    fn zerocopy_detects_sequence_gap() {
        let buf = make_update(1_000, 2_000, 1, 5);
        let result = validate_pool_update(&buf, 3); // expected seq=4, got seq=5
        assert_eq!(result, Err(ValidationError::SequenceGap { expected: 4, got: 5 }));
    }

    #[test]
    fn zerocopy_rejects_short_slice() {
        let short = [0u8; 10];
        assert_eq!(validate_pool_update(&short, 0), Err(ValidationError::TooShort));
    }

    #[test]
    fn no_copy_same_pointer() {
        // Verify zerocopy: the returned reference points into the original buffer.
        let buf = make_update(42_000, 84_000, 1, 1);
        let update = validate_pool_update(&buf, 0).unwrap();
        // The update's bytes-as-slice must overlap buf.
        let buf_ptr = buf.as_ptr() as usize;
        let update_ptr = update as *const _ as usize;
        assert_eq!(update_ptr, buf_ptr, "zerocopy must alias original buffer");
    }
}
