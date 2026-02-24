use bytemuck::{Pod, Zeroable};

/// POD wire payload designed for bytemuck pointer casts in hot path.
///
/// All numeric fields are explicitly encoded as little-endian byte arrays to
/// guarantee deterministic parsing across architectures.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DexSwapTx {
    pub nonce_le: [u8; 8],
    pub pool_address: [u8; 20],
    pub amount_in_le: [u8; 8],
    pub min_amount_out_le: [u8; 8],
    pub token_direction: u8,
    pub _reserved: [u8; 3],
}

impl DexSwapTx {
    pub const WIRE_SIZE: usize = core::mem::size_of::<DexSwapTx>();

    #[inline(always)]
    pub fn nonce(&self) -> u64 {
        u64::from_le_bytes(self.nonce_le)
    }

    #[inline(always)]
    pub fn amount_in(&self) -> u64 {
        u64::from_le_bytes(self.amount_in_le)
    }

    #[inline(always)]
    pub fn min_amount_out(&self) -> u64 {
        u64::from_le_bytes(self.min_amount_out_le)
    }

    #[inline(always)]
    pub fn from_parts(
        nonce: u64,
        pool_address: [u8; 20],
        amount_in: u64,
        min_amount_out: u64,
        token_direction: u8,
    ) -> Self {
        Self {
            nonce_le: nonce.to_le_bytes(),
            pool_address,
            amount_in_le: amount_in.to_le_bytes(),
            min_amount_out_le: min_amount_out.to_le_bytes(),
            token_direction,
            _reserved: [0; 3],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DexSwapTx;
    use bytemuck::bytes_of;

    #[test]
    fn parses_little_endian_fields_correctly() {
        let tx = DexSwapTx::from_parts(0x0102_0304_0506_0708, [0xAA; 20], 1_500_000, 1_490_000, 1);
        let bytes = bytes_of(&tx);
        let parsed = bytemuck::try_from_bytes::<DexSwapTx>(&bytes[..DexSwapTx::WIRE_SIZE])
            .expect("wire payload must map into DexSwapTx");

        assert_eq!(parsed.nonce(), 0x0102_0304_0506_0708);
        assert_eq!(parsed.amount_in(), 1_500_000);
        assert_eq!(parsed.min_amount_out(), 1_490_000);
        assert_eq!(parsed.token_direction, 1);
    }

    #[test]
    fn round_trip_wire_bytes() {
        let tx = DexSwapTx::from_parts(77, [0xAB; 20], 2_000_000, 1_980_000, 0);
        let raw = bytemuck::bytes_of(&tx);
        let parsed =
            bytemuck::try_from_bytes::<DexSwapTx>(raw).expect("serialized payload must parse back");

        assert_eq!(parsed.nonce(), 77);
        assert_eq!(parsed.amount_in(), 2_000_000);
        assert_eq!(parsed.min_amount_out(), 1_980_000);
    }
}
