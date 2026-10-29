use crate::config::{CURRENT_PROTOCOL_VERSION, DEFAULT_DISCOVERY_PORT, DEFAULT_MULTICAST_ADDR};
use crate::errors::AppResult;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

/// Public announcement packet broadcast via UDP Multicast (`224.0.0.167:53317`)
/// inspired by LocalSend Protocol v2.
/// Strictly excludes any chat content, project paths, usernames, or tokens!
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryAnnouncement {
    pub app: String,
    pub protocol_version: String,
    pub session_id: String,
    pub device_alias: String,
    pub os: String,
    pub arch: String,
    pub https_port: u16,
    pub cert_fingerprint_prefix: String,
    pub pake_salt_hex: String,
    pub full_cert_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPeer {
    pub session_id: String,
    pub device_alias: String,
    pub os: String,
    pub arch: String,
    pub ip_address: String,
    pub https_port: u16,
    pub cert_fingerprint_prefix: String,
    pub full_cert_fingerprint: String,
    pub pake_salt_hex: String,
    pub last_seen_ms_ago: u64,
}

pub struct DiscoveryServiceHandle {
    stop_flag: Arc<AtomicBool>,
    peers: Arc<Mutex<HashMap<String, (DiscoveryAnnouncement, String, Instant)>>>,
}

impl DiscoveryServiceHandle {
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    pub fn list_active_peers(&self) -> Vec<DiscoveredPeer> {
        let Ok(guard) = self.peers.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (_, (ann, ip, seen_at)) in guard.iter() {
            let elapsed = seen_at.elapsed();
            if elapsed <= Duration::from_secs(15) {
                out.push(DiscoveredPeer {
                    session_id: ann.session_id.clone(),
                    device_alias: ann.device_alias.clone(),
                    os: ann.os.clone(),
                    arch: ann.arch.clone(),
                    ip_address: ip.clone(),
                    https_port: ann.https_port,
                    cert_fingerprint_prefix: ann.cert_fingerprint_prefix.clone(),
                    full_cert_fingerprint: ann.full_cert_fingerprint.clone(),
                    pake_salt_hex: ann.pake_salt_hex.clone(),
                    last_seen_ms_ago: elapsed.as_millis() as u64,
                });
            }
        }
        out.sort_by(|a, b| a.device_alias.cmp(&b.device_alias));
        out
    }
}

impl Drop for DiscoveryServiceHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts ephemeral LAN discovery (listener + optional broadcaster when acting as receiver).
/// Runs only while the user is on the connection/pairing screen.
pub fn start_lan_discovery(
    own_announcement: Option<DiscoveryAnnouncement>,
    discovery_port: u16,
) -> AppResult<DiscoveryServiceHandle> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let peers: Arc<Mutex<HashMap<String, (DiscoveryAnnouncement, String, Instant)>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let multicast_ip: Ipv4Addr = DEFAULT_MULTICAST_ADDR
        .parse()
        .unwrap_or(Ipv4Addr::new(224, 0, 0, 167));

    // 1. Spawn UDP Listener thread
    {
        let stop_clone = stop_flag.clone();
        let peers_clone = peers.clone();
        let own_session_id = own_announcement.as_ref().map(|a| a.session_id.clone());

        std::thread::spawn(move || {
            let Ok(socket) = create_reusable_multicast_socket(discovery_port, multicast_ip) else {
                return;
            };
            let _ = socket.set_read_timeout(Some(Duration::from_millis(400)));
            let mut buf = [0u8; 4096];

            while !stop_clone.load(Ordering::SeqCst) {
                if let Ok((n, src_addr)) = socket.recv_from(&mut buf) {
                    if let Ok(ann) = serde_json::from_slice::<DiscoveryAnnouncement>(&buf[..n]) {
                        if ann.app == "codex-move"
                            && Some(&ann.session_id) != own_session_id.as_ref()
                        {
                            if let Ok(mut guard) = peers_clone.lock() {
                                guard.insert(
                                    ann.session_id.clone(),
                                    (ann, src_addr.ip().to_string(), Instant::now()),
                                );
                            }
                        }
                    }
                }
            }
        });
    }

    // 2. If acting as Receiver, broadcast UDP announcement every 1.5 seconds until stopped
    if let Some(ann) = own_announcement {
        let stop_clone = stop_flag.clone();
        std::thread::spawn(move || {
            let Ok(send_sock) = UdpSocket::bind("0.0.0.0:0") else {
                return;
            };
            let _ = send_sock.set_broadcast(true);
            let _ = send_sock.set_multicast_loop_v4(true);
            let Ok(payload) = serde_json::to_vec(&ann) else {
                return;
            };

            let multicast_dest = SocketAddr::new(IpAddr::V4(multicast_ip), discovery_port);
            let loopback_dest =
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), discovery_port);

            while !stop_clone.load(Ordering::SeqCst) {
                let _ = send_sock.send_to(&payload, multicast_dest);
                let _ = send_sock.send_to(&payload, loopback_dest);
                std::thread::sleep(Duration::from_millis(1200));
            }
        });
    }

    Ok(DiscoveryServiceHandle { stop_flag, peers })
}

fn create_reusable_multicast_socket(
    port: u16,
    multicast_ip: Ipv4Addr,
) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    let _ = socket.set_reuse_address(true);
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    socket.bind(&bind_addr.into())?;
    let _ = socket.join_multicast_v4(&multicast_ip, &Ipv4Addr::UNSPECIFIED);
    let _ = socket.set_multicast_loop_v4(true);
    Ok(socket.into())
}

/// Detects the primary non-loopback LAN IPv4 address of this machine.
pub fn detect_local_lan_ip() -> String {
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("192.0.2.1:80").is_ok() {
            if let Ok(local_addr) = sock.local_addr() {
                return local_addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

pub fn build_receiver_announcement(
    session_id: &str,
    device_alias: &str,
    https_port: u16,
    full_cert_fingerprint: &str,
    pake_salt_hex: &str,
) -> DiscoveryAnnouncement {
    let prefix = full_cert_fingerprint
        .chars()
        .take(12)
        .collect::<String>()
        .to_ascii_uppercase();
    DiscoveryAnnouncement {
        app: "codex-move".to_string(),
        protocol_version: CURRENT_PROTOCOL_VERSION.to_string(),
        session_id: session_id.to_string(),
        device_alias: device_alias.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        https_port,
        cert_fingerprint_prefix: prefix,
        pake_salt_hex: pake_salt_hex.to_string(),
        full_cert_fingerprint: full_cert_fingerprint.to_string(),
    }
}
