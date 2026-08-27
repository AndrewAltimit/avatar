//! `avatar-osc` — the VRChat **OSC avatar-parameter** runtime layer (M5).
//!
//! This crate speaks VRChat's OSC *parameter* protocol: the `/avatar/parameters/*`,
//! `/avatar/change`, and `/input/*` address spaces a running VRChat client sends and accepts. It is
//! the foundation under the analog-gesture daemon (`avatar-osc-gestures`, PLAN §4) and any tool that
//! wants to drive or observe an avatar at runtime.
//!
//! It is **not** the VMC tracker protocol (raw `/tracking/*` transforms) — that lives in
//! `avatar_input::osc` and feeds the rig layer. The two are deliberately separate address spaces:
//! tracker transforms drive a local rig; avatar parameters drive a VRChat avatar.
//!
//! ## Shape
//!
//! - [`codec`] is a **pure** model of the wire format — [`ParamMessage`], [`AvatarChange`],
//!   [`InputMessage`] (axes/buttons) encode to / decode from `rosc::OscMessage` with no I/O, so the
//!   protocol is unit-tested in isolation. Same split as the tracker backend.
//! - [`query`] parses an avatar's OSCQuery config JSON ([`AvatarConfig`]) — its parameter schema
//!   (names, OSC type tags, read/write access) — entirely offline.
//! - [`ParamClient`] is the thin UDP transport: it serializes codec messages to VRChat
//!   (default `127.0.0.1:9000`) and polls incoming parameter updates (default listen `:9001`),
//!   non-blocking. The transport wraps the codec; the codec never touches a socket.
//!
//! ```no_run
//! use avatar_osc::{ParamClient, ParamValue};
//! let mut client = ParamClient::connect_default()?;
//! client.send_param("VRCEmote", ParamValue::Int(3))?;       // wave
//! for update in client.poll()? {                            // drain pending updates
//!     println!("{} = {:?}", update.name, update.value);
//! }
//! # anyhow::Ok(())
//! ```
//!
//! References: <https://docs.vrchat.com/docs/osc-overview>,
//! <https://docs.vrchat.com/docs/osc-avatar-parameters>,
//! <https://docs.vrchat.com/docs/osc-as-input-controller>.

// Regression guard for an ingest crate: an `.unwrap()`/`.expect()` on a parse/decode path turns
// malformed wire bytes or config into an opaque panic instead of a structured `anyhow` error an agent
// can read. Warn on them in non-test code — CI runs clippy with `-D warnings`, so a new one fails the
// build; tests use them freely.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

use std::io;
use std::net::{ToSocketAddrs, UdpSocket};

use anyhow::{Context, Result};
use rosc::{OscMessage, OscPacket};

pub mod capture;
pub mod codec;
pub mod oscquery;
pub mod query;

pub use codec::{
    AVATAR_CHANGE, AvatarChange, INPUT_PREFIX, InputAxis, InputButton, InputMessage, PARAM_PREFIX,
    ParamMessage, ParamValue,
};
pub use query::{Access, AvatarConfig, AvatarParam};

/// VRChat's default OSC ports: it **listens** on 9000 (we send here) and **sends** on 9001 (we
/// listen here). <https://docs.vrchat.com/docs/osc-overview#how-to-use-osc-in-vrchat>.
pub const VRCHAT_RECV_PORT: u16 = 9000;
pub const VRCHAT_SEND_PORT: u16 = 9001;

/// A UDP transport over the VRChat OSC parameter protocol.
///
/// Send is fire-and-forget (UDP); receive is non-blocking and poll-style — [`poll`](Self::poll)
/// drains every datagram queued since the last call and returns the parameter updates it found,
/// mirroring `avatar_input::osc::OscSource::poll`. All wire (de)serialization delegates to the pure
/// [`codec`]; this type only owns the socket.
pub struct ParamClient {
    socket: UdpSocket,
    /// Where VRChat listens — the destination for our sends.
    target: std::net::SocketAddr,
    buf: Vec<u8>,
}

impl ParamClient {
    /// Bind a non-blocking receive socket at `listen` and aim sends at `target`.
    ///
    /// `listen` is where *we* receive VRChat's outgoing parameters (VRChat's send port, default
    /// `127.0.0.1:9001`); `target` is where VRChat listens for our messages (default
    /// `127.0.0.1:9000`).
    pub fn new<L: ToSocketAddrs, T: ToSocketAddrs>(listen: L, target: T) -> Result<ParamClient> {
        let socket = UdpSocket::bind(listen).context("binding OSC receive socket")?;
        socket
            .set_nonblocking(true)
            .context("setting OSC socket non-blocking")?;
        let target = target
            .to_socket_addrs()
            .context("resolving OSC target address")?
            .next()
            .context("OSC target address resolved to nothing")?;
        Ok(ParamClient {
            socket,
            target,
            buf: vec![0u8; 65_535],
        })
    }

    /// Connect with VRChat's default ports — receive on `127.0.0.1:9001`, send to `127.0.0.1:9000`.
    pub fn connect_default() -> Result<ParamClient> {
        ParamClient::new(
            ("127.0.0.1", VRCHAT_SEND_PORT),
            ("127.0.0.1", VRCHAT_RECV_PORT),
        )
    }

    /// The address sends are aimed at (where VRChat listens).
    pub fn target(&self) -> std::net::SocketAddr {
        self.target
    }

    /// The local address the receive socket is bound to (useful when bound to port 0 and the
    /// real port must be advertised, e.g. over OSCQuery).
    pub fn local_addr(&self) -> Result<std::net::SocketAddr> {
        self.socket.local_addr().context("OSC socket local addr")
    }

    /// Encode and send one OSC message to VRChat.
    fn send_message(&self, msg: OscMessage) -> Result<()> {
        let bytes = rosc::encoder::encode(&OscPacket::Message(msg))
            .map_err(|e| anyhow::anyhow!("encoding OSC message: {e:?}"))?;
        self.socket
            .send_to(&bytes, self.target)
            .context("sending OSC datagram")?;
        Ok(())
    }

    /// Set an avatar parameter: `/avatar/parameters/<name>` with a typed value.
    pub fn send_param(&self, name: impl Into<String>, value: ParamValue) -> Result<()> {
        self.send_message(ParamMessage::new(name, value).to_osc())
    }

    /// Send an axis input (`/input/<Axis>`, float clamped to `-1..=1`).
    pub fn send_axis(&self, axis: InputAxis, value: f32) -> Result<()> {
        self.send_message(InputMessage::Axis(axis, value).to_osc())
    }

    /// Send a button input (`/input/<Button>`, `1`/`0`). For a momentary tap, send `true` then
    /// `false` — VRChat holds the button until it sees `0` (reset-to-zero semantics).
    pub fn send_button(&self, button: InputButton, pressed: bool) -> Result<()> {
        self.send_message(InputMessage::Button(button, pressed).to_osc())
    }

    /// Request VRChat load an avatar by blueprint id (`/avatar/change`).
    pub fn send_avatar_change(&self, id: impl Into<String>) -> Result<()> {
        self.send_message(
            AvatarChange {
                id: id.into(),
                config_path: None,
            }
            .to_osc(),
        )
    }

    /// Drain every datagram queued since the last call and return the avatar-parameter updates in
    /// them. Non-parameter messages (input echoes, `/avatar/change`, malformed payloads) are
    /// skipped rather than erroring, so a noisy socket never breaks the poll. Returns an error only
    /// on a genuine socket failure (not `WouldBlock`).
    pub fn poll(&mut self) -> Result<Vec<ParamMessage>> {
        let mut out = Vec::new();
        loop {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, _)) => {
                    if let Ok((_, packet)) = rosc::decoder::decode_udp(&self.buf[..n]) {
                        collect_params(&packet, &mut out);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e).context("receiving OSC datagram"),
            }
        }
        Ok(out)
    }
}

/// Pull every `/avatar/parameters/*` update out of an OSC packet (recursing into bundles). Pure —
/// the unit of testing for the receive path.
pub fn collect_params(packet: &OscPacket, out: &mut Vec<ParamMessage>) {
    match packet {
        OscPacket::Message(m) => {
            if let Ok(Some(p)) = ParamMessage::from_osc(m) {
                out.push(p);
            }
        }
        OscPacket::Bundle(b) => {
            for p in &b.content {
                collect_params(p, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{OscBundle, OscTime, OscType};

    #[test]
    fn collect_params_pulls_only_avatar_params() {
        let bundle = OscPacket::Bundle(OscBundle {
            timetag: OscTime::from((0, 0)),
            content: vec![
                OscPacket::Message(OscMessage {
                    addr: "/avatar/parameters/VRCEmote".to_string(),
                    args: vec![OscType::Int(2)],
                }),
                OscPacket::Message(OscMessage {
                    addr: "/avatar/change".to_string(),
                    args: vec![OscType::String("avtr_x".to_string())],
                }),
                OscPacket::Message(OscMessage {
                    addr: "/input/Jump".to_string(),
                    args: vec![OscType::Int(1)],
                }),
            ],
        });
        let mut out = Vec::new();
        collect_params(&bundle, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "VRCEmote");
        assert_eq!(out[0].value, ParamValue::Int(2));
    }

    #[test]
    fn send_and_receive_over_loopback() {
        // Bind two clients cross-wired on ephemeral ports and round-trip a parameter over real UDP.
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();
        drop(a);
        drop(b);

        let sender = ParamClient::new(a_addr, b_addr).unwrap();
        let mut receiver = ParamClient::new(b_addr, a_addr).unwrap();

        sender
            .send_param("Grounded", ParamValue::Bool(true))
            .unwrap();
        // Give the datagram a moment; loopback is effectively immediate but poll is non-blocking.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let updates = receiver.poll().unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].name, "Grounded");
        assert_eq!(updates[0].value, ParamValue::Bool(true));
    }

    #[test]
    fn poll_is_empty_when_nothing_arrives() {
        let mut client = ParamClient::new("127.0.0.1:0", ("127.0.0.1", VRCHAT_RECV_PORT)).unwrap();
        assert!(client.poll().unwrap().is_empty());
    }

    #[test]
    fn default_target_port_is_9000() {
        let client = ParamClient::new("127.0.0.1:0", ("127.0.0.1", VRCHAT_RECV_PORT)).unwrap();
        assert_eq!(client.target().port(), 9000);
    }
}
