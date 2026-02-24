use heapless::spsc::Queue;

/// Cache-aligned wrapper to reduce false sharing across producer/consumer.
#[repr(align(64))]
pub struct CacheAligned<T>(pub T);

pub struct ResponseRing<const N: usize> {
    inner: CacheAligned<Queue<[u8; 8], N>>,
}

impl<const N: usize> ResponseRing<N> {
    pub fn new() -> Self {
        Self {
            inner: CacheAligned(Queue::new()),
        }
    }

    #[inline(always)]
    pub fn enqueue(&mut self, value: [u8; 8]) -> Result<(), [u8; 8]> {
        self.inner.0.enqueue(value)
    }

    #[inline(always)]
    pub fn dequeue(&mut self) -> Option<[u8; 8]> {
        self.inner.0.dequeue()
    }
}

impl<const N: usize> Default for ResponseRing<N> {
    fn default() -> Self {
        Self::new()
    }
}
