#[cfg(any(target_os = "linux", target_os = "android"))]
mod linux_node {
    use mev_zerocopy_node::affinity;
    use mev_zerocopy_node::processor;
    use mev_zerocopy_node::ring::ResponseRing;
    use mev_zerocopy_node::runtime::{LatencyClock, NodeStats};
    use mev_zerocopy_node::xdp::{self, XdpConfig};
    use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
    use smoltcp::phy::{Medium, TunTapInterface};
    use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer as TcpSocketBuffer};
    use smoltcp::socket::udp::{
        PacketBuffer as UdpPacketBuffer, PacketMetadata as UdpPacketMetadata, Socket as UdpSocket,
    };
    use smoltcp::time::Instant;
    use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, IpEndpoint};

    fn init_runtime() {
        env_logger::init();
        if affinity::pin_current_thread_to(0) {
            log::info!("Pinned processing thread to CPU core 0");
        } else {
            log::warn!("CPU pinning failed or unavailable");
        }
    }

    fn backend_mode() -> &'static str {
        match std::env::var("MEV_BACKEND") {
            Ok(v) if v.eq_ignore_ascii_case("af_xdp") => "af_xdp",
            _ => "tap",
        }
    }

    pub fn run() {
        init_runtime();

        let stats = NodeStats::new();
        let mut response_ring: ResponseRing<1024> = ResponseRing::new();

        if backend_mode() == "af_xdp" {
            let cfg = XdpConfig::default();
            let available = xdp::probe_af_xdp_socket();
            log::info!(
                "AF_XDP requested: iface={}, queue={}, mode={:?}, available={}",
                cfg.interface,
                cfg.queue_id,
                cfg.mode,
                available
            );
            if !available {
                log::warn!("AF_XDP socket probe failed, falling back to TAP transport");
            }
        }

        log::info!("Starting MEV node with smoltcp userspace stack");

        let tap_name = "tap0";
        let mut device = TunTapInterface::new(tap_name, Medium::Ethernet)
            .expect("failed to open tap0; run scripts/setup_tap.sh first");

        let hardware_addr = EthernetAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
        let mut iface = Interface::new(
            Config::new(hardware_addr.into()),
            &mut device,
            Instant::now(),
        );
        iface.update_ip_addrs(|ip_addrs| {
            ip_addrs
                .push(IpCidr::new(IpAddress::v4(192, 168, 69, 2), 24))
                .unwrap();
        });

        let mut socket_storage = [SocketStorage::EMPTY, SocketStorage::EMPTY];
        let mut sockets = SocketSet::new(&mut socket_storage[..]);

        let mut tcp_rx = [0u8; 65_535];
        let mut tcp_tx = [0u8; 65_535];
        let tcp_socket = TcpSocket::new(
            TcpSocketBuffer::new(&mut tcp_rx[..]),
            TcpSocketBuffer::new(&mut tcp_tx[..]),
        );
        let tcp_handle = sockets.add(tcp_socket);

        let mut udp_rx_meta = [UdpPacketMetadata::EMPTY; 64];
        let mut udp_tx_meta = [UdpPacketMetadata::EMPTY; 64];
        let mut udp_rx_payload = [0u8; 16 * 1024];
        let mut udp_tx_payload = [0u8; 16 * 1024];
        let udp_socket = UdpSocket::new(
            UdpPacketBuffer::new(&mut udp_rx_meta[..], &mut udp_rx_payload[..]),
            UdpPacketBuffer::new(&mut udp_tx_meta[..], &mut udp_tx_payload[..]),
        );
        let udp_handle = sockets.add(udp_socket);

        log::info!("Listening on 192.168.69.2:8080 (TCP+UDP via smoltcp)");

        loop {
            let now = Instant::now();
            iface.poll(now, &mut device, &mut sockets);

            {
                let udp = sockets.get_mut::<UdpSocket>(udp_handle);
                if !udp.is_open() {
                    udp.bind(8080).expect("udp bind failed");
                }

                if udp.can_recv() {
                    let latency = LatencyClock::start();
                    if let Ok((payload, meta)) = udp.recv() {
                        stats.rx_packets.inc();
                        if let Some(profit) = processor::process_packet(payload) {
                            stats.opportunities.inc();
                            let _ = response_ring.enqueue(profit.to_le_bytes());
                            if let Some(reply) = response_ring.dequeue() {
                                let remote =
                                    IpEndpoint::new(meta.endpoint.addr, meta.endpoint.port);
                                let _ = udp.send_slice(&reply, remote);
                                stats.tx_packets.inc();
                            }
                        }
                    }
                    let sample = latency.stop();
                    log::debug!(
                        "UDP hot-path latency: {} cycles / {} us",
                        sample.cycles,
                        sample.micros
                    );
                }
            }

            {
                let tcp = sockets.get_mut::<TcpSocket>(tcp_handle);
                if !tcp.is_open() {
                    tcp.listen(8080).expect("tcp listen failed");
                }

                if tcp.can_recv() {
                    let latency = LatencyClock::start();
                    if let Ok(maybe_profit) =
                        tcp.recv(|payload| (payload.len(), processor::process_packet(payload)))
                    {
                        stats.rx_packets.inc();
                        if let Some(profit) = maybe_profit {
                            stats.opportunities.inc();
                            let _ = response_ring.enqueue(profit.to_le_bytes());
                            if let Some(reply) = response_ring.dequeue() {
                                if tcp.can_send() {
                                    let _ = tcp.send_slice(&reply);
                                    stats.tx_packets.inc();
                                }
                            }
                        }
                    }
                    let sample = latency.stop();
                    log::debug!(
                        "TCP hot-path latency: {} cycles / {} us",
                        sample.cycles,
                        sample.micros
                    );
                }
            }

            if stats.rx_packets.load() % 100_000 == 0 && stats.rx_packets.load() != 0 {
                log::info!(
                    "stats: rx={}, tx={}, opps={}",
                    stats.rx_packets.load(),
                    stats.tx_packets.load(),
                    stats.opportunities.load()
                );
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn main() {
    linux_node::run();
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn main() {
    eprintln!(
        "This node requires Linux/Android kernel networking features. Use Linux runtime for AF_XDP/TAP paths."
    );
}
