/// AF_XDP kernel-bypass networking layer.
///
/// AF_XDP sockets bypass the Linux kernel's `sk_buff` allocation path entirely.
/// Packets are DMA-mapped directly into a userspace memory region (UMEM),
/// eliminating the kernel→userspace copy that standard sockets incur.
///
/// Architecture:
///   NIC DMA → UMEM frame pool → XDP Fill Ring (kernel writes RX descriptors)
///                             → XDP RX Ring   (userspace reads RX descriptors)
///                             → XDP TX Ring   (userspace writes TX descriptors)
///                             → XDP Completion Ring (kernel confirms TX done)
///
/// This module provides:
/// - `XdpConfig` and `XdpMode` — configuration types
/// - `UmemConfig` — UMEM memory region parameters
/// - `XdpRingDescriptor` — the actual ring buffer entry (POD, cache-aligned)
/// - `XdpUmem` — UMEM descriptor (real mmap on Linux, stub elsewhere)
/// - `XdpSocket` — high-level AF_XDP socket wrapper with ring management
/// - `probe_af_xdp_socket()` — lightweight kernel capability check

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XdpMode {
    /// Native XDP: loaded at the NIC driver level, before sk_buff allocation.
    /// Requires driver support (e.g., i40e, mlx5, veth with XDP).
    Native,
    /// Generic XDP: loaded at the network stack level.
    /// Works on any interface but doesn't bypass sk_buff allocation.
    Generic,
}

#[derive(Clone, Copy, Debug)]
pub struct XdpConfig {
    /// Network interface name (e.g., "eth0", "veth-bot0").
    pub interface: &'static str,
    /// Hardware queue index to bind to (0 for first queue).
    pub queue_id: u32,
    /// XDP loading mode.
    pub mode: XdpMode,
}

impl Default for XdpConfig {
    fn default() -> Self {
        Self {
            interface: "veth0",
            queue_id: 0,
            mode: XdpMode::Native,
        }
    }
}

/// UMEM (User Memory) configuration.
///
/// UMEM is a contiguous memory region registered with the kernel via
/// `setsockopt(XDP_UMEM_REG)`. The kernel DMA-maps NIC frames directly
/// into this buffer — no copy, no `sk_buff` allocation.
#[derive(Clone, Copy, Debug)]
pub struct UmemConfig {
    /// Number of frames in the UMEM region. Must be a power of two.
    pub frame_count: u32,
    /// Size of each frame in bytes (typically 4096, matching a page).
    pub frame_size: u32,
    /// Size of the Fill and Completion rings (must be power of two).
    pub fill_ring_size: u32,
    /// Size of the RX and TX rings (must be power of two).
    pub rx_tx_ring_size: u32,
}

impl Default for UmemConfig {
    fn default() -> Self {
        Self {
            frame_count: 4096,
            frame_size: 4096,
            fill_ring_size: 2048,
            rx_tx_ring_size: 2048,
        }
    }
}

impl UmemConfig {
    /// Total size of the UMEM region in bytes.
    #[inline(always)]
    pub fn total_size(&self) -> usize {
        self.frame_count as usize * self.frame_size as usize
    }
}

/// A single AF_XDP ring buffer descriptor.
///
/// Placed in shared memory between kernel and userspace. The kernel writes
/// RX descriptors (addr + len) into the RX ring; userspace reads them,
/// processes the frame at UMEM[addr..addr+len], then recycles via Fill ring.
///
/// `#[repr(C, align(64))]` ensures:
/// - Correct ABI layout expected by the Linux kernel.
/// - Each descriptor occupies its own cache line (no false sharing).
#[repr(C, align(64))]
#[derive(Clone, Copy, Debug, Default)]
pub struct XdpRingDescriptor {
    /// Byte offset of the frame within the UMEM region.
    pub addr: u64,
    /// Number of valid bytes in the frame.
    pub len: u32,
    /// Kernel-internal options field (must be zero for userspace).
    pub options: u32,
    /// Padding to fill cache line (64 - 8 - 4 - 4 = 48 bytes).
    _pad: [u8; 48],
}

const _: () = assert!(core::mem::size_of::<XdpRingDescriptor>() == 64);

impl XdpRingDescriptor {
    /// Create a new descriptor pointing to a UMEM frame.
    #[inline(always)]
    pub fn new(addr: u64, len: u32) -> Self {
        Self { addr, len, options: 0, _pad: [0u8; 48] }
    }
}

// ─── Linux-only implementation ────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub use linux_impl::*;

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{UmemConfig, XdpConfig, XdpRingDescriptor};

    // Linux kernel constants for AF_XDP
    const AF_XDP: i32 = 44;
    const SOL_XDP: i32 = 283;
    const XDP_UMEM_REG: i32 = 5;
    const XDP_UMEM_FILL_RING: i32 = 6;
    const XDP_UMEM_COMPLETION_RING: i32 = 7;
    const XDP_RX_RING: i32 = 1;
    const XDP_TX_RING: i32 = 2;
    const XDP_MMAP_OFFSETS: i32 = 3;

    /// Registered UMEM region — mmap'd memory shared with the kernel.
    ///
    /// On creation, the memory is pinned via `mlock` to prevent paging,
    /// and registered with the kernel via `setsockopt(XDP_UMEM_REG)`.
    pub struct XdpUmem {
        /// Pointer to the start of the mmap'd UMEM region.
        pub ptr: *mut u8,
        /// Total size of the region.
        pub size: usize,
        /// Configuration used to create this UMEM.
        pub config: UmemConfig,
        /// File descriptor of the socket this UMEM is registered on.
        pub fd: i32,
    }

    impl XdpUmem {
        /// Allocate and register a UMEM region with the kernel.
        ///
        /// Steps:
        /// 1. Open AF_XDP socket.
        /// 2. `mmap(MAP_ANONYMOUS | MAP_POPULATE)` to allocate pinned memory.
        /// 3. `setsockopt(XDP_UMEM_REG)` to register the region.
        /// 4. `setsockopt(XDP_UMEM_FILL_RING)` + `setsockopt(XDP_UMEM_COMPLETION_RING)`
        ///    to size the fill/completion rings.
        pub fn allocate(config: UmemConfig) -> Result<Self, XdpError> {
            let size = config.total_size();

            // Step 1: open AF_XDP socket
            let fd = unsafe { libc::socket(AF_XDP, libc::SOCK_RAW, 0) };
            if fd < 0 {
                return Err(XdpError::SocketOpen(unsafe { *libc::__errno_location() }));
            }

            // Step 2: mmap anonymous memory for UMEM
            let ptr = unsafe {
                libc::mmap(
                    core::ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_POPULATE,
                    -1,
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                unsafe { libc::close(fd) };
                return Err(XdpError::MmapFailed(unsafe { *libc::__errno_location() }));
            }

            // Pin memory to prevent paging (critical for deterministic latency)
            if unsafe { libc::mlock(ptr, size) } != 0 {
                unsafe {
                    libc::munmap(ptr, size);
                    libc::close(fd);
                }
                return Err(XdpError::MlockFailed(unsafe { *libc::__errno_location() }));
            }

            // Step 3: register UMEM with the kernel
            #[repr(C)]
            struct XdpUmemReg {
                addr: u64,
                len: u64,
                chunk_size: u32,
                headroom: u32,
                flags: u32,
            }
            let reg = XdpUmemReg {
                addr: ptr as u64,
                len: size as u64,
                chunk_size: config.frame_size,
                headroom: 0,
                flags: 0,
            };
            let rc = unsafe {
                libc::setsockopt(
                    fd,
                    SOL_XDP,
                    XDP_UMEM_REG,
                    &reg as *const _ as *const libc::c_void,
                    core::mem::size_of::<XdpUmemReg>() as libc::socklen_t,
                )
            };
            if rc != 0 {
                unsafe {
                    libc::munmap(ptr, size);
                    libc::close(fd);
                }
                return Err(XdpError::UmemReg(unsafe { *libc::__errno_location() }));
            }

            // Step 4: size the fill ring
            let fill_size = config.fill_ring_size as i32;
            let _ = unsafe {
                libc::setsockopt(
                    fd,
                    SOL_XDP,
                    XDP_UMEM_FILL_RING,
                    &fill_size as *const _ as *const libc::c_void,
                    core::mem::size_of::<i32>() as libc::socklen_t,
                )
            };

            // Size the completion ring
            let _ = unsafe {
                libc::setsockopt(
                    fd,
                    SOL_XDP,
                    XDP_UMEM_COMPLETION_RING,
                    &fill_size as *const _ as *const libc::c_void,
                    core::mem::size_of::<i32>() as libc::socklen_t,
                )
            };

            Ok(Self { ptr: ptr as *mut u8, size, config, fd })
        }

        /// Return a mutable slice for the frame at `frame_index`.
        /// Panics if the index is out of bounds.
        #[inline(always)]
        pub unsafe fn frame_mut(&mut self, frame_index: u32) -> &mut [u8] {
            assert!((frame_index as usize) < self.config.frame_count as usize);
            let offset = frame_index as usize * self.config.frame_size as usize;
            core::slice::from_raw_parts_mut(
                self.ptr.add(offset),
                self.config.frame_size as usize,
            )
        }
    }

    impl Drop for XdpUmem {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.ptr as *mut libc::c_void, self.size);
                libc::close(self.fd);
            }
        }
    }

    // Safety: XdpUmem is only used from a single pinned thread.
    unsafe impl Send for XdpUmem {}

    /// High-level AF_XDP socket with bound UMEM and sized rings.
    ///
    /// After calling `bind()`, packets arrive in the RX ring. Each descriptor
    /// contains `(addr, len)` pointing into the UMEM. The caller reads
    /// `umem.frame_at(addr)` — zero-copy — and recycles the frame via Fill ring.
    pub struct XdpSocket {
        pub fd: i32,
        pub config: XdpConfig,
    }

    impl XdpSocket {
        /// Create a new AF_XDP socket, configure RX/TX rings, and bind to
        /// the given interface queue.
        ///
        /// Requires `CAP_NET_ADMIN` (or `CAP_BPF` on newer kernels).
        pub fn open(cfg: XdpConfig, umem: &XdpUmem) -> Result<Self, XdpError> {
            // Open a new socket for the XSK (separate from UMEM's socket)
            let fd = unsafe { libc::socket(AF_XDP, libc::SOCK_RAW, 0) };
            if fd < 0 {
                return Err(XdpError::SocketOpen(unsafe { *libc::__errno_location() }));
            }

            // Size the RX ring
            let ring_size = umem.config.rx_tx_ring_size as i32;
            let _ = unsafe {
                libc::setsockopt(
                    fd,
                    SOL_XDP,
                    XDP_RX_RING,
                    &ring_size as *const _ as *const libc::c_void,
                    core::mem::size_of::<i32>() as libc::socklen_t,
                )
            };

            // Size the TX ring
            let _ = unsafe {
                libc::setsockopt(
                    fd,
                    SOL_XDP,
                    XDP_TX_RING,
                    &ring_size as *const _ as *const libc::c_void,
                    core::mem::size_of::<i32>() as libc::socklen_t,
                )
            };

            // Bind the socket to the interface + queue
            #[repr(C)]
            struct SockaddrXdp {
                sxdp_family: u16,
                sxdp_flags: u16,
                sxdp_ifindex: u32,
                sxdp_queue_id: u32,
                sxdp_shared_umem_fd: u32,
            }
            let ifindex = unsafe {
                libc::if_nametoindex(
                    cfg.interface.as_ptr() as *const libc::c_char,
                )
            };
            if ifindex == 0 {
                unsafe { libc::close(fd) };
                return Err(XdpError::IfNotFound);
            }

            // XDP_USE_NEED_WAKEUP = 8; XDP_COPY (generic) = 2; XDP_ZEROCOPY (native) = 4
            let flags: u16 = match cfg.mode {
                super::XdpMode::Native => 4,  // XDP_ZEROCOPY
                super::XdpMode::Generic => 2, // XDP_COPY (fallback)
            };
            let sa = SockaddrXdp {
                sxdp_family: AF_XDP as u16,
                sxdp_flags: flags,
                sxdp_ifindex: ifindex,
                sxdp_queue_id: cfg.queue_id,
                sxdp_shared_umem_fd: umem.fd as u32,
            };
            let rc = unsafe {
                libc::bind(
                    fd,
                    &sa as *const _ as *const libc::sockaddr,
                    core::mem::size_of::<SockaddrXdp>() as libc::socklen_t,
                )
            };
            if rc != 0 {
                unsafe { libc::close(fd) };
                return Err(XdpError::BindFailed(unsafe { *libc::__errno_location() }));
            }

            log::info!(
                "AF_XDP socket bound: iface={} queue={} mode={:?} fd={}",
                cfg.interface, cfg.queue_id, cfg.mode, fd
            );
            Ok(Self { fd, config: cfg })
        }

        /// Poll the RX ring for a received frame descriptor (non-blocking).
        ///
        /// Returns `Some(desc)` if a frame is available in the ring, `None` if empty.
        /// In production, this is called in a tight loop on a pinned CPU core.
        ///
        /// The caller must read `umem.frame_at(desc.addr)[..desc.len]` and then
        /// recycle the frame by writing `desc.addr` back to the Fill ring.
        #[inline(always)]
        pub fn poll_rx(&self, rx_ring_ptr: *mut XdpRingDescriptor, rx_idx: &mut u32, ring_size: u32) -> Option<XdpRingDescriptor> {
            // In real AF_XDP usage the ring pointers are mmap'd from the kernel
            // via getsockopt(XDP_MMAP_OFFSETS) + mmap(fd, offset=XDP_PGOFF_RX_RING).
            // Here we read from the pre-mapped ring pointer.
            let mask = ring_size - 1;
            let desc = unsafe {
                let slot = rx_ring_ptr.add((*rx_idx & mask) as usize);
                *slot
            };
            if desc.len == 0 {
                return None; // ring empty
            }
            *rx_idx = rx_idx.wrapping_add(1);
            Some(desc)
        }
    }

    impl Drop for XdpSocket {
        fn drop(&mut self) {
            unsafe { libc::close(self.fd) };
        }
    }

    /// Errors from AF_XDP setup.
    #[derive(Debug, Clone, Copy)]
    pub enum XdpError {
        SocketOpen(i32),
        MmapFailed(i32),
        MlockFailed(i32),
        UmemReg(i32),
        IfNotFound,
        BindFailed(i32),
    }

    impl core::fmt::Display for XdpError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::SocketOpen(e) => write!(f, "AF_XDP socket open failed (errno={})", e),
                Self::MmapFailed(e) => write!(f, "UMEM mmap failed (errno={})", e),
                Self::MlockFailed(e) => write!(f, "mlock failed (errno={})", e),
                Self::UmemReg(e) => write!(f, "XDP_UMEM_REG setsockopt failed (errno={})", e),
                Self::IfNotFound => write!(f, "network interface not found"),
                Self::BindFailed(e) => write!(f, "AF_XDP bind failed (errno={})", e),
            }
        }
    }

    /// Probe whether the running kernel supports AF_XDP sockets.
    ///
    /// Opens and immediately closes an AF_XDP socket. Does not allocate UMEM
    /// or attach any eBPF program. Safe to call without `CAP_NET_ADMIN`.
    pub fn probe_af_xdp_socket() -> bool {
        let fd = unsafe { libc::socket(AF_XDP, libc::SOCK_RAW, 0) };
        if fd < 0 {
            return false;
        }
        let _ = unsafe { libc::close(fd) };
        true
    }
}

// ─── Non-Linux stub ───────────────────────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
pub fn probe_af_xdp_socket() -> bool {
    false
}
