//! OSC tracker backend (behind the `osc` feature).
//!
//! Receives transform messages over UDP and folds them into a [`TrackerState`]. The address scheme
//! is deliberately simple and transform-oriented (raw poses, *not* VRChat `/avatar/parameters/*`):
//!
//! | Address                       | Args                                                    |
//! |-------------------------------|---------------------------------------------------------|
//! | `/tracking/hmd`               | `f×7`: pos xyz, quat xyzw                                |
//! | `/tracking/controller/left`   | `f×7` pose, then `trigger grip stickX stickY` (`f×4`), optional `int` buttons |
//! | `/tracking/controller/right`  | as left                                                 |
//! | `/tracking/tracker/<n>`       | `f×7`: pos xyz, quat xyzw                                |
//!
//! The decode of one message into the state ([`apply_message`]) is a pure function, unit-tested
//! without a socket; the UDP receive loop is a thin non-blocking wrapper.

use std::io;
use std::net::{ToSocketAddrs, UdpSocket};

use glam::{Quat, Vec3};
use rosc::{OscPacket, OscType};

use crate::{Controller, Pose6dof, TrackerSource, TrackerState};

/// A live OSC backend: binds a UDP socket and drains pending datagrams on each [`poll`].
///
/// [`poll`]: TrackerSource::poll
pub struct OscSource {
    socket: UdpSocket,
    state: TrackerState,
    buf: Vec<u8>,
}

impl OscSource {
    /// Bind a non-blocking UDP socket (e.g. `"127.0.0.1:9000"`).
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(OscSource {
            socket,
            state: TrackerState::default(),
            buf: vec![0u8; 65_535],
        })
    }
}

impl TrackerSource for OscSource {
    fn poll(&mut self) -> TrackerState {
        // Drain everything queued; keep only the resulting (latest) state.
        loop {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, _)) => {
                    if let Ok((_, packet)) = rosc::decoder::decode_udp(&self.buf[..n]) {
                        apply_packet(&mut self.state, &packet);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        self.state.clone()
    }
}

/// Fold an OSC packet (message or bundle) into the tracking state.
pub fn apply_packet(state: &mut TrackerState, packet: &OscPacket) {
    match packet {
        OscPacket::Message(m) => apply_message(state, &m.addr, &m.args),
        OscPacket::Bundle(b) => {
            for p in &b.content {
                apply_packet(state, p);
            }
        }
    }
}

/// Fold one OSC message (by address + args) into the tracking state. Pure — the unit of testing.
pub fn apply_message(state: &mut TrackerState, addr: &str, args: &[OscType]) {
    match addr {
        "/tracking/hmd" => {
            if let Some(p) = read_pose(args) {
                state.hmd = p;
            }
        }
        "/tracking/controller/left" => {
            if let Some(c) = read_controller(args) {
                state.left = c;
            }
        }
        "/tracking/controller/right" => {
            if let Some(c) = read_controller(args) {
                state.right = c;
            }
        }
        _ => {
            if let Some(rest) = addr.strip_prefix("/tracking/tracker/")
                && let Ok(idx) = rest.parse::<usize>()
                && let Some(p) = read_pose(args)
            {
                if state.trackers.len() <= idx {
                    state.trackers.resize(idx + 1, Pose6dof::default());
                }
                state.trackers[idx] = p;
            }
        }
    }
}

/// Leading floating-point args, accepting both `Float` and `Double`.
fn floats(args: &[OscType]) -> Vec<f32> {
    args.iter()
        .filter_map(|a| match a {
            OscType::Float(f) => Some(*f),
            OscType::Double(d) => Some(*d as f32),
            _ => None,
        })
        .collect()
}

fn read_pose(args: &[OscType]) -> Option<Pose6dof> {
    let f = floats(args);
    if f.len() < 7 {
        return None;
    }
    Some(Pose6dof {
        position: Vec3::new(f[0], f[1], f[2]),
        orientation: Quat::from_xyzw(f[3], f[4], f[5], f[6]).normalize(),
    })
}

fn read_controller(args: &[OscType]) -> Option<Controller> {
    let pose = read_pose(args)?;
    let f = floats(args);
    let buttons = args
        .iter()
        .find_map(|a| match a {
            OscType::Int(i) => Some(*i as u32),
            _ => None,
        })
        .unwrap_or(0);
    Some(Controller {
        pose,
        trigger: f.get(7).copied().unwrap_or(0.0),
        grip: f.get(8).copied().unwrap_or(0.0),
        stick: [
            f.get(9).copied().unwrap_or(0.0),
            f.get(10).copied().unwrap_or(0.0),
        ],
        buttons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_hmd_pose() {
        let mut state = TrackerState::default();
        let args = vec![
            OscType::Float(1.0),
            OscType::Float(2.0),
            OscType::Float(3.0),
            OscType::Float(0.0),
            OscType::Float(0.0),
            OscType::Float(0.0),
            OscType::Float(1.0),
        ];
        apply_message(&mut state, "/tracking/hmd", &args);
        assert_eq!(state.hmd.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(state.hmd.orientation, Quat::IDENTITY);
    }

    #[test]
    fn decodes_controller_with_analog_and_buttons() {
        let mut state = TrackerState::default();
        let mut args: Vec<OscType> = (0..7).map(|_| OscType::Float(0.0)).collect();
        args[6] = OscType::Float(1.0); // quat w = 1
        args.push(OscType::Float(0.8)); // trigger
        args.push(OscType::Float(0.5)); // grip
        args.push(OscType::Float(-1.0)); // stick x
        args.push(OscType::Float(0.25)); // stick y
        args.push(OscType::Int(0b101)); // buttons
        apply_message(&mut state, "/tracking/controller/right", &args);
        assert_eq!(state.right.trigger, 0.8);
        assert_eq!(state.right.grip, 0.5);
        assert_eq!(state.right.stick, [-1.0, 0.25]);
        assert_eq!(state.right.buttons, 0b101);
    }

    #[test]
    fn decodes_indexed_tracker_and_grows_vec() {
        let mut state = TrackerState::default();
        let args = vec![
            OscType::Float(0.0),
            OscType::Float(0.9),
            OscType::Float(0.0),
            OscType::Float(0.0),
            OscType::Float(0.0),
            OscType::Float(0.0),
            OscType::Float(1.0),
        ];
        apply_message(&mut state, "/tracking/tracker/2", &args);
        assert_eq!(state.trackers.len(), 3, "vec grew to hold index 2");
        assert_eq!(state.trackers[2].position, Vec3::new(0.0, 0.9, 0.0));
    }

    #[test]
    fn short_message_is_ignored() {
        let mut state = TrackerState::default();
        apply_message(&mut state, "/tracking/hmd", &[OscType::Float(1.0)]);
        assert_eq!(state.hmd, Pose6dof::default(), "too few floats → no update");
    }
}
