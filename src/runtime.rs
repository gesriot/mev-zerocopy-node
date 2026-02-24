use core::sync::atomic::{AtomicU64, Ordering};
use minstant::Instant;

#[repr(align(64))]
pub struct CacheAlignedAtomicU64(pub AtomicU64);

impl CacheAlignedAtomicU64 {
    pub const fn new(v: u64) -> Self {
        Self(AtomicU64::new(v))
    }

    #[inline(always)]
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn load(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

pub struct NodeStats {
    pub rx_packets: CacheAlignedAtomicU64,
    pub tx_packets: CacheAlignedAtomicU64,
    pub opportunities: CacheAlignedAtomicU64,
}

impl NodeStats {
    pub const fn new() -> Self {
        Self {
            rx_packets: CacheAlignedAtomicU64::new(0),
            tx_packets: CacheAlignedAtomicU64::new(0),
            opportunities: CacheAlignedAtomicU64::new(0),
        }
    }
}

impl Default for NodeStats {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LatencySample {
    pub cycles: u64,
    pub micros: u64,
}

pub struct LatencyClock {
    start_cycles: u64,
    start_time: Instant,
}

impl LatencyClock {
    #[inline(always)]
    pub fn start() -> Self {
        Self {
            start_cycles: rdtsc(),
            start_time: Instant::now(),
        }
    }

    #[inline(always)]
    pub fn stop(self) -> LatencySample {
        let cycles = rdtsc().saturating_sub(self.start_cycles);
        let micros = self.start_time.elapsed().as_micros() as u64;
        LatencySample { cycles, micros }
    }
}

#[inline(always)]
fn rdtsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_rdtsc()
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}
