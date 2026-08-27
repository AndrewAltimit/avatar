//! Minimal **OSCQuery advertisement**, VRChat flavor — the modern discovery handshake that
//! replaces "VRChat always sends to 9001".
//!
//! Since 2023 VRChat discovers OSC apps over mDNS: a service advertises `_oscjson._tcp.local`,
//! VRChat fetches `http://<addr>/?HOST_INFO` from it, reads `OSC_PORT`, checks the service's
//! parameter tree for an `/avatar` node, and then **sends its avatar-parameter output straight
//! to that port** — whatever it is. A listener that only binds the legacy 9001 misses traffic
//! whenever the port is taken or the client is routing to discovered services, so the capture
//! tools advertise themselves instead of guessing.
//!
//! Scope: same-host VRChat (the advertised address is `127.0.0.1`), advertise-only (we do not
//! browse or resolve other services beyond noting VRChat's own announcements as a diagnostic).
//! The DNS packets are hand-built and hand-parsed like every other format in this repo — the
//! subset mDNS needs is a 12-byte header plus labelled names, and the responder answers exactly
//! one question: `PTR _oscjson._tcp.local`.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The mDNS multicast group / port.
const MDNS_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
/// The service type VRChat browses for.
const SERVICE: &str = "_oscjson._tcp.local";

/// A running advertisement: an HTTP responder answering the OSCQuery handshake and an mDNS
/// responder answering (and periodically announcing) the service record. Dropping it stops both.
pub struct OscQueryAdvertiser {
    name: String,
    http_port: u16,
    stop: Arc<AtomicBool>,
    /// Diagnostics from the mDNS thread: names of VRChat's own OSCQuery announcements seen.
    vrchat_seen: mpsc::Receiver<String>,
}

impl OscQueryAdvertiser {
    /// Start advertising `name` (sanitized to DNS-label characters) as an OSCQuery service whose
    /// OSC endpoint is `127.0.0.1:<osc_port>`.
    pub fn start(name: &str, osc_port: u16) -> Result<OscQueryAdvertiser> {
        let name: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let http = TcpListener::bind(("127.0.0.1", 0)).context("binding OSCQuery HTTP socket")?;
        let http_port = http.local_addr().context("HTTP local addr")?.port();
        http.set_nonblocking(true)
            .context("setting HTTP socket non-blocking")?;

        let stop = Arc::new(AtomicBool::new(false));
        let (seen_tx, seen_rx) = mpsc::channel();

        // HTTP thread: answer every request — HOST_INFO or the parameter tree.
        {
            let stop = stop.clone();
            let name = name.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match http.accept() {
                        Ok((mut conn, _)) => {
                            let mut buf = [0u8; 2048];
                            let n = conn.read(&mut buf).unwrap_or(0);
                            let req = String::from_utf8_lossy(&buf[..n]);
                            let body = if req.lines().next().unwrap_or("").contains("HOST_INFO") {
                                host_info_json(&name, osc_port)
                            } else {
                                tree_json(&name)
                            };
                            let _ = conn.write_all(
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                )
                                .as_bytes(),
                            );
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(200)),
                    }
                }
            });
        }

        // mDNS thread: answer PTR queries for the service type, announce periodically, and note
        // VRChat's own announcements as a diagnostic.
        {
            let stop = stop.clone();
            let name = name.clone();
            std::thread::spawn(move || {
                let Ok(socket) = mdns_socket() else {
                    let _ = seen_tx.send(
                        "(mDNS bind failed — another responder holds 5353 exclusively)".into(),
                    );
                    return;
                };
                let response = build_response(&name, http_port);
                let mut next_announce = Instant::now();
                let mut buf = [0u8; 4096];
                while !stop.load(Ordering::Relaxed) {
                    if Instant::now() >= next_announce {
                        let _ = socket.send_to(&response, (MDNS_ADDR, MDNS_PORT));
                        next_announce = Instant::now() + Duration::from_secs(15);
                    }
                    match socket.recv_from(&mut buf) {
                        Ok((n, from)) => {
                            let pkt = &buf[..n];
                            if packet_queries_service(pkt) {
                                let _ = socket.send_to(&response, (MDNS_ADDR, MDNS_PORT));
                                let _ = socket.send_to(&response, from);
                            }
                            if let Some(inst) = vrchat_instance_in(pkt) {
                                let _ = seen_tx.send(inst);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(200)),
                    }
                }
            });
        }

        Ok(OscQueryAdvertiser {
            name,
            http_port,
            stop,
            vrchat_seen: seen_rx,
        })
    }

    /// The advertised service name (sanitized).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The HTTP port the OSCQuery handshake answers on.
    pub fn http_port(&self) -> u16 {
        self.http_port
    }

    /// Drain diagnostics: instance names of VRChat's own OSCQuery service seen on mDNS since the
    /// last call (deduplicated by the caller if desired).
    pub fn vrchat_announcements(&self) -> Vec<String> {
        self.vrchat_seen.try_iter().collect()
    }
}

impl Drop for OscQueryAdvertiser {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The `?HOST_INFO` handshake document: where our OSC endpoint is.
fn host_info_json(name: &str, osc_port: u16) -> String {
    format!(
        r#"{{"NAME":"{name}","OSC_IP":"127.0.0.1","OSC_PORT":{osc_port},"OSC_TRANSPORT":"UDP","EXTENSIONS":{{"ACCESS":true,"VALUE":true,"DESCRIPTION":true}}}}"#
    )
}

/// The parameter tree: advertising an `/avatar` node is what tells VRChat to route its
/// avatar-parameter output (and `/avatar/change`) to this service.
fn tree_json(name: &str) -> String {
    format!(
        r#"{{"DESCRIPTION":"{name}","FULL_PATH":"/","ACCESS":0,"CONTENTS":{{"avatar":{{"FULL_PATH":"/avatar","ACCESS":2,"CONTENTS":{{"change":{{"FULL_PATH":"/avatar/change","TYPE":"s","ACCESS":2}},"parameters":{{"FULL_PATH":"/avatar/parameters","ACCESS":2,"CONTENTS":{{}}}}}}}}}}}}"#
    )
}

/// Build the mDNS socket: port 5353, address reuse (the OS resolver usually shares it),
/// multicast membership, non-blocking.
fn mdns_socket() -> Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .context("creating mDNS socket")?;
    s.set_reuse_address(true).context("SO_REUSEADDR")?;
    // SO_REUSEPORT needs the `all` feature on unix; SO_REUSEADDR alone shares 5353 with the
    // system responder on both platforms we care about (Windows on the rig, Linux in dev).

    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, MDNS_PORT)).into())
        .context("binding 5353")?;
    s.join_multicast_v4(&MDNS_ADDR, &Ipv4Addr::UNSPECIFIED)
        .context("joining mDNS multicast group")?;
    s.set_nonblocking(true).context("non-blocking")?;
    Ok(s.into())
}

// ---- DNS wire helpers (the subset mDNS needs) ----------------------------------------------

/// Append a DNS name (dot-separated labels, uncompressed).
fn put_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

/// Append one resource record.
fn put_record(
    out: &mut Vec<u8>,
    name: &str,
    rtype: u16,
    cache_flush: bool,
    ttl: u32,
    rdata: &[u8],
) {
    put_name(out, name);
    out.extend_from_slice(&rtype.to_be_bytes());
    let class: u16 = if cache_flush { 0x8001 } else { 0x0001 };
    out.extend_from_slice(&class.to_be_bytes());
    out.extend_from_slice(&ttl.to_be_bytes());
    out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    out.extend_from_slice(rdata);
}

/// The full announcement/response: PTR (service → instance), SRV (instance → host:port),
/// TXT, and the host's A record (127.0.0.1).
fn build_response(instance: &str, http_port: u16) -> Vec<u8> {
    let inst_name = format!("{instance}.{SERVICE}");
    let host_name = format!("{instance}.local");
    let mut out = Vec::with_capacity(256);
    // Header: response, authoritative; 4 answers.
    out.extend_from_slice(&0u16.to_be_bytes()); // ID
    out.extend_from_slice(&0x8400u16.to_be_bytes()); // QR | AA
    out.extend_from_slice(&0u16.to_be_bytes()); // QD
    out.extend_from_slice(&4u16.to_be_bytes()); // AN
    out.extend_from_slice(&0u16.to_be_bytes()); // NS
    out.extend_from_slice(&0u16.to_be_bytes()); // AR

    let mut rdata = Vec::new();
    put_name(&mut rdata, &inst_name);
    put_record(&mut out, SERVICE, 12, false, 4500, &rdata); // PTR (shared — no cache-flush)

    rdata.clear();
    rdata.extend_from_slice(&0u16.to_be_bytes()); // priority
    rdata.extend_from_slice(&0u16.to_be_bytes()); // weight
    rdata.extend_from_slice(&http_port.to_be_bytes());
    put_name(&mut rdata, &host_name);
    put_record(&mut out, &inst_name, 33, true, 120, &rdata); // SRV

    put_record(
        &mut out,
        &inst_name,
        16,
        true,
        4500,
        &[9, b't', b'x', b't', b'v', b'e', b'r', b's', b'=', b'1'],
    ); // TXT

    put_record(
        &mut out,
        &host_name,
        1,
        true,
        120,
        &Ipv4Addr::LOCALHOST.octets(),
    ); // A

    out
}

/// Read a (possibly compression-pointed) DNS name starting at `pos`; returns the dotted name
/// lowercased and the position just after it (in the uncompressed stream).
fn read_name(pkt: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut jumped_end: Option<usize> = None;
    let mut hops = 0;
    loop {
        let len = *pkt.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | *pkt.get(pos + 1)? as usize;
            if jumped_end.is_none() {
                jumped_end = Some(pos + 2);
            }
            pos = ptr;
            hops += 1;
            if hops > 8 {
                return None; // pointer loop
            }
            continue;
        }
        let label = pkt.get(pos + 1..pos + 1 + len)?;
        labels.push(String::from_utf8_lossy(label).to_lowercase());
        pos += 1 + len;
    }
    Some((labels.join("."), jumped_end.unwrap_or(pos)))
}

/// Does this packet's question section ask for our service type (PTR/ANY)?
fn packet_queries_service(pkt: &[u8]) -> bool {
    let Some(header) = pkt.get(..12) else {
        return false;
    };
    let flags = u16::from_be_bytes([header[2], header[3]]);
    if flags & 0x8000 != 0 {
        return false; // a response, not a query
    }
    let qdcount = u16::from_be_bytes([header[4], header[5]]) as usize;
    let mut pos = 12;
    for _ in 0..qdcount.min(16) {
        let Some((name, after)) = read_name(pkt, pos) else {
            return false;
        };
        let Some(qtype) = pkt.get(after..after + 2) else {
            return false;
        };
        let qtype = u16::from_be_bytes([qtype[0], qtype[1]]);
        if name == SERVICE && (qtype == 12 || qtype == 255) {
            return true;
        }
        pos = after + 4;
    }
    false
}

/// If this packet is a response carrying records for a `VRChat-Client-*` OSCQuery instance,
/// return that instance name (a diagnostic: VRChat's OSCQuery side is alive on this host).
fn vrchat_instance_in(pkt: &[u8]) -> Option<String> {
    let header = pkt.get(..12)?;
    let flags = u16::from_be_bytes([header[2], header[3]]);
    if flags & 0x8000 == 0 {
        return None; // a query
    }
    let qdcount = u16::from_be_bytes([header[4], header[5]]) as usize;
    let ancount = u16::from_be_bytes([header[6], header[7]]) as usize;
    let mut pos = 12;
    for _ in 0..qdcount.min(16) {
        let (_, after) = read_name(pkt, pos)?;
        pos = after + 4;
    }
    for _ in 0..ancount.min(32) {
        let (name, after) = read_name(pkt, pos)?;
        let rdlen = u16::from_be_bytes([*pkt.get(after + 8)?, *pkt.get(after + 9)?]) as usize;
        if name.starts_with("vrchat-client") && name.ends_with(SERVICE) {
            return Some(name);
        }
        // PTR rdata can also carry the instance name.
        if name == SERVICE
            && let Some((target, _)) = read_name(pkt, after + 10)
            && target.starts_with("vrchat-client")
        {
            return Some(target);
        }
        pos = after + 10 + rdlen;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Our own response parses back: the PTR question matcher sees a query for the service, and
    /// the VRChat detector finds a VRChat-Client instance in a response shaped like ours.
    #[test]
    fn dns_roundtrip_and_matchers() {
        // A query packet: header + one PTR question for the service type.
        let mut q = Vec::new();
        q.extend_from_slice(&0u16.to_be_bytes());
        q.extend_from_slice(&0u16.to_be_bytes()); // flags: query
        q.extend_from_slice(&1u16.to_be_bytes());
        q.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        put_name(&mut q, SERVICE);
        q.extend_from_slice(&12u16.to_be_bytes());
        q.extend_from_slice(&1u16.to_be_bytes());
        assert!(packet_queries_service(&q));
        assert!(
            vrchat_instance_in(&q).is_none(),
            "queries are not announcements"
        );

        // Our response is not mistaken for a query, and carries our (non-VRChat) instance.
        let r = build_response("avatar-capture", 8080);
        assert!(!packet_queries_service(&r));
        assert!(vrchat_instance_in(&r).is_none());

        // A VRChat-shaped response is detected.
        let v = build_response("VRChat-Client-ABC123", 9001);
        assert_eq!(
            vrchat_instance_in(&v).as_deref(),
            Some("vrchat-client-abc123._oscjson._tcp.local")
        );

        // Name reader handles compression pointers: point back at the service name in `q`.
        let mut c = q.clone();
        let ptr_pos = c.len();
        c.extend_from_slice(&[0xC0, 12]); // pointer to offset 12 (the question name)
        let (name, after) = read_name(&c, ptr_pos).unwrap();
        assert_eq!(name, SERVICE);
        assert_eq!(after, ptr_pos + 2);
    }

    /// The handshake JSON is valid and carries the OSC endpoint + an /avatar node.
    #[test]
    fn handshake_json_is_valid() {
        let hi: serde_json::Value = serde_json::from_str(&host_info_json("cap", 9012)).unwrap();
        assert_eq!(hi["OSC_PORT"], 9012);
        assert_eq!(hi["OSC_IP"], "127.0.0.1");
        let tree: serde_json::Value = serde_json::from_str(&tree_json("cap")).unwrap();
        assert_eq!(tree["CONTENTS"]["avatar"]["FULL_PATH"], "/avatar");
        assert!(tree["CONTENTS"]["avatar"]["CONTENTS"]["parameters"].is_object());
    }
}
