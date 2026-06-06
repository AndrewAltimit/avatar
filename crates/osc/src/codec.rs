//! Pure codec for the VRChat OSC **avatar-parameter** and **input** address spaces.
//!
//! Everything here is a pure function over `rosc` types — no socket, no I/O — so the wire format is
//! unit-tested in isolation and the transport ([`crate::ParamClient`]) is a thin wrapper. This is
//! the same split the tracker backend uses (`avatar_input::osc`), but for the *parameter* protocol
//! (`/avatar/parameters/*`, `/avatar/change`, `/input/*`) rather than raw tracker transforms.
//!
//! References: <https://docs.vrchat.com/docs/osc-overview>,
//! <https://docs.vrchat.com/docs/osc-avatar-parameters>,
//! <https://docs.vrchat.com/docs/osc-as-input-controller>.

use anyhow::{Result, bail};
use rosc::{OscMessage, OscType};

/// Address prefix every avatar parameter lives under: `/avatar/parameters/<Name>`.
pub const PARAM_PREFIX: &str = "/avatar/parameters/";
/// The avatar-change broadcast VRChat sends (and accepts): `/avatar/change`.
pub const AVATAR_CHANGE: &str = "/avatar/change";
/// Address prefix for the input controller: `/input/<Name>`.
pub const INPUT_PREFIX: &str = "/input/";

/// A typed value for an avatar parameter. VRChat exposes exactly three scalar types over OSC —
/// `bool`, `int` (`0..=255`, sent as an OSC `i`), and `float` (`-1.0..=1.0`, sent as an OSC `f`) —
/// matching the three Avatars-3.0 parameter types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamValue {
    Bool(bool),
    /// VRChat int parameters are byte-ranged; we keep the full `i32` on the wire and let callers
    /// clamp/validate, but model the intent with `i32`.
    Int(i32),
    Float(f32),
}

impl ParamValue {
    /// The single OSC argument this value serializes to.
    pub fn to_osc(self) -> OscType {
        match self {
            ParamValue::Bool(b) => OscType::Bool(b),
            ParamValue::Int(i) => OscType::Int(i),
            ParamValue::Float(f) => OscType::Float(f),
        }
    }

    /// Recover a [`ParamValue`] from a single OSC argument. VRChat is lenient about bool-vs-int on
    /// the wire (some senders emit `T`/`F`, some `1`/`0`), so we accept the obvious coercions but
    /// preserve the declared type where the tag is unambiguous.
    pub fn from_osc(arg: &OscType) -> Option<ParamValue> {
        match arg {
            OscType::Bool(b) => Some(ParamValue::Bool(*b)),
            OscType::Int(i) => Some(ParamValue::Int(*i)),
            OscType::Float(f) => Some(ParamValue::Float(*f)),
            OscType::Double(d) => Some(ParamValue::Float(*d as f32)),
            OscType::Long(l) => Some(ParamValue::Int(*l as i32)),
            _ => None,
        }
    }

    /// The OSC type tag (`"T"`/`"F"` collapse to `"b"` here for schema purposes) — `"b"`, `"i"`,
    /// `"f"`. Matches the tags an OSCQuery config reports for the same parameter.
    pub fn type_tag(self) -> &'static str {
        match self {
            ParamValue::Bool(_) => "b",
            ParamValue::Int(_) => "i",
            ParamValue::Float(_) => "f",
        }
    }
}

/// One avatar-parameter update: the parameter *name* (the bit after `/avatar/parameters/`) plus its
/// typed value. This is the unit both directions of the protocol speak in.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamMessage {
    pub name: String,
    pub value: ParamValue,
}

impl ParamMessage {
    pub fn new(name: impl Into<String>, value: ParamValue) -> ParamMessage {
        ParamMessage {
            name: name.into(),
            value,
        }
    }

    /// Build the `rosc` message `/avatar/parameters/<name>` with a single typed argument.
    pub fn to_osc(&self) -> OscMessage {
        OscMessage {
            addr: format!("{PARAM_PREFIX}{}", self.name),
            args: vec![self.value.to_osc()],
        }
    }

    /// Parse a `/avatar/parameters/<name>` message back into a [`ParamMessage`]. Returns `Ok(None)`
    /// for any other address (so a caller can fall through to [`AvatarChange`] / input parsing);
    /// errors only when the address *is* a parameter address but the payload is malformed.
    pub fn from_osc(msg: &OscMessage) -> Result<Option<ParamMessage>> {
        let Some(name) = msg.addr.strip_prefix(PARAM_PREFIX) else {
            return Ok(None);
        };
        if name.is_empty() {
            bail!("parameter address {:?} has an empty name", msg.addr);
        }
        let Some(arg) = msg.args.first() else {
            bail!("parameter {name:?} carried no OSC argument");
        };
        let Some(value) = ParamValue::from_osc(arg) else {
            bail!("parameter {name:?} carried unsupported OSC type {arg:?}");
        };
        Ok(Some(ParamMessage {
            name: name.to_string(),
            value,
        }))
    }
}

/// The `/avatar/change` event: VRChat broadcasts it when the local user switches avatars, carrying
/// the new avatar's blueprint id and the path of its OSC config file on disk. We can also *send* it
/// to ask VRChat to load a specific avatar.
#[derive(Debug, Clone, PartialEq)]
pub struct AvatarChange {
    /// Avatar blueprint id, e.g. `avtr_xxxxxxxx-xxxx-...`.
    pub id: String,
    /// Filesystem path of the avatar's generated OSC config JSON, if present in the message.
    pub config_path: Option<String>,
}

impl AvatarChange {
    /// Build the `rosc` message `/avatar/change` with the id (and config path when known).
    pub fn to_osc(&self) -> OscMessage {
        let mut args = vec![OscType::String(self.id.clone())];
        if let Some(path) = &self.config_path {
            args.push(OscType::String(path.clone()));
        }
        OscMessage {
            addr: AVATAR_CHANGE.to_string(),
            args,
        }
    }

    /// Parse `/avatar/change`. `Ok(None)` for any other address; errors only on a malformed payload.
    pub fn from_osc(msg: &OscMessage) -> Result<Option<AvatarChange>> {
        if msg.addr != AVATAR_CHANGE {
            return Ok(None);
        }
        let id = match msg.args.first() {
            Some(OscType::String(s)) => s.clone(),
            other => bail!("/avatar/change expected a string id, got {other:?}"),
        };
        let config_path = match msg.args.get(1) {
            Some(OscType::String(s)) => Some(s.clone()),
            _ => None,
        };
        Ok(Some(AvatarChange { id, config_path }))
    }
}

/// The canonical VRChat input axes — continuous controls in `-1.0..=1.0` sent as `/input/<Axis>`
/// with one float argument. (Movement, look, and the GoGo locomotion analogs.)
///
/// Source: <https://docs.vrchat.com/docs/osc-as-input-controller>.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAxis {
    Vertical,
    Horizontal,
    LookHorizontal,
    LookVertical,
    MoveHold,
    SpinHold,
    UseAxisRight,
    GrabAxisRight,
    MoveHoldFB,
    SpinHoldCwCcw,
    SpinHoldUD,
    SpinHoldLR,
}

impl InputAxis {
    /// The address suffix (the bit after `/input/`).
    pub fn name(self) -> &'static str {
        match self {
            InputAxis::Vertical => "Vertical",
            InputAxis::Horizontal => "Horizontal",
            InputAxis::LookHorizontal => "LookHorizontal",
            InputAxis::LookVertical => "LookVertical",
            InputAxis::MoveHold => "MoveHold",
            InputAxis::SpinHold => "SpinHold",
            InputAxis::UseAxisRight => "UseAxisRight",
            InputAxis::GrabAxisRight => "GrabAxisRight",
            InputAxis::MoveHoldFB => "MoveHoldFB",
            InputAxis::SpinHoldCwCcw => "SpinHoldCwCcw",
            InputAxis::SpinHoldUD => "SpinHoldUD",
            InputAxis::SpinHoldLR => "SpinHoldLR",
        }
    }

    /// Resolve an axis from its `/input/` suffix.
    pub fn from_name(name: &str) -> Option<InputAxis> {
        Some(match name {
            "Vertical" => InputAxis::Vertical,
            "Horizontal" => InputAxis::Horizontal,
            "LookHorizontal" => InputAxis::LookHorizontal,
            "LookVertical" => InputAxis::LookVertical,
            "MoveHold" => InputAxis::MoveHold,
            "SpinHold" => InputAxis::SpinHold,
            "UseAxisRight" => InputAxis::UseAxisRight,
            "GrabAxisRight" => InputAxis::GrabAxisRight,
            "MoveHoldFB" => InputAxis::MoveHoldFB,
            "SpinHoldCwCcw" => InputAxis::SpinHoldCwCcw,
            "SpinHoldUD" => InputAxis::SpinHoldUD,
            "SpinHoldLR" => InputAxis::SpinHoldLR,
            _ => return None,
        })
    }
}

/// The canonical VRChat input buttons — momentary controls sent as `/input/<Button>` with one int
/// argument (`1` = pressed, `0` = released). VRChat treats these like held keys: a button stays
/// active until you send `0`, so a "tap" is `1` then `0` (the reset-to-zero semantics).
///
/// Source: <https://docs.vrchat.com/docs/osc-as-input-controller>.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputButton {
    MoveForward,
    MoveBackward,
    MoveLeft,
    MoveRight,
    LookLeft,
    LookRight,
    Jump,
    Run,
    ComfortLeft,
    ComfortRight,
    DropRight,
    UseRight,
    GrabRight,
    DropLeft,
    UseLeft,
    GrabLeft,
    PanicButton,
    QuickMenuToggleLeft,
    QuickMenuToggleRight,
    Voice,
}

impl InputButton {
    /// The address suffix (the bit after `/input/`).
    pub fn name(self) -> &'static str {
        match self {
            InputButton::MoveForward => "MoveForward",
            InputButton::MoveBackward => "MoveBackward",
            InputButton::MoveLeft => "MoveLeft",
            InputButton::MoveRight => "MoveRight",
            InputButton::LookLeft => "LookLeft",
            InputButton::LookRight => "LookRight",
            InputButton::Jump => "Jump",
            InputButton::Run => "Run",
            InputButton::ComfortLeft => "ComfortLeft",
            InputButton::ComfortRight => "ComfortRight",
            InputButton::DropRight => "DropRight",
            InputButton::UseRight => "UseRight",
            InputButton::GrabRight => "GrabRight",
            InputButton::DropLeft => "DropLeft",
            InputButton::UseLeft => "UseLeft",
            InputButton::GrabLeft => "GrabLeft",
            InputButton::PanicButton => "PanicButton",
            InputButton::QuickMenuToggleLeft => "QuickMenuToggleLeft",
            InputButton::QuickMenuToggleRight => "QuickMenuToggleRight",
            InputButton::Voice => "Voice",
        }
    }

    /// Resolve a button from its `/input/` suffix.
    pub fn from_name(name: &str) -> Option<InputButton> {
        Some(match name {
            "MoveForward" => InputButton::MoveForward,
            "MoveBackward" => InputButton::MoveBackward,
            "MoveLeft" => InputButton::MoveLeft,
            "MoveRight" => InputButton::MoveRight,
            "LookLeft" => InputButton::LookLeft,
            "LookRight" => InputButton::LookRight,
            "Jump" => InputButton::Jump,
            "Run" => InputButton::Run,
            "ComfortLeft" => InputButton::ComfortLeft,
            "ComfortRight" => InputButton::ComfortRight,
            "DropRight" => InputButton::DropRight,
            "UseRight" => InputButton::UseRight,
            "GrabRight" => InputButton::GrabRight,
            "DropLeft" => InputButton::DropLeft,
            "UseLeft" => InputButton::UseLeft,
            "GrabLeft" => InputButton::GrabLeft,
            "PanicButton" => InputButton::PanicButton,
            "QuickMenuToggleLeft" => InputButton::QuickMenuToggleLeft,
            "QuickMenuToggleRight" => InputButton::QuickMenuToggleRight,
            "Voice" => InputButton::Voice,
            _ => return None,
        })
    }
}

/// One input-controller command: either an axis at a float position or a button at a pressed state.
/// VRChat puts both under `/input/<Name>`, distinguished only by argument type (`f` vs `i`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputMessage {
    /// `/input/<Axis>` with a float, clamped by the encoder to `-1.0..=1.0`.
    Axis(InputAxis, f32),
    /// `/input/<Button>` with `1` (pressed) or `0` (released).
    Button(InputButton, bool),
}

impl InputMessage {
    /// Build the `rosc` `/input/<Name>` message. Axis floats are clamped to VRChat's `-1..=1`.
    pub fn to_osc(&self) -> OscMessage {
        match self {
            InputMessage::Axis(axis, v) => OscMessage {
                addr: format!("{INPUT_PREFIX}{}", axis.name()),
                args: vec![OscType::Float(v.clamp(-1.0, 1.0))],
            },
            InputMessage::Button(btn, pressed) => OscMessage {
                addr: format!("{INPUT_PREFIX}{}", btn.name()),
                args: vec![OscType::Int(if *pressed { 1 } else { 0 })],
            },
        }
    }

    /// Parse an `/input/<Name>` message. `Ok(None)` for any other address or any `/input/` suffix
    /// that isn't a known axis/button; errors only when a recognised address has a wrong/missing
    /// argument. The float-vs-int tag is what disambiguates axis from button when a name could be
    /// either (none currently overlap, but the tag is authoritative).
    pub fn from_osc(msg: &OscMessage) -> Result<Option<InputMessage>> {
        let Some(suffix) = msg.addr.strip_prefix(INPUT_PREFIX) else {
            return Ok(None);
        };
        if let Some(axis) = InputAxis::from_name(suffix) {
            return match msg.args.first() {
                Some(OscType::Float(f)) => Ok(Some(InputMessage::Axis(axis, *f))),
                Some(OscType::Int(i)) => Ok(Some(InputMessage::Axis(axis, *i as f32))),
                other => bail!("input axis {suffix:?} expected a float, got {other:?}"),
            };
        }
        if let Some(btn) = InputButton::from_name(suffix) {
            return match msg.args.first() {
                Some(OscType::Int(i)) => Ok(Some(InputMessage::Button(btn, *i != 0))),
                Some(OscType::Bool(b)) => Ok(Some(InputMessage::Button(btn, *b))),
                Some(OscType::Float(f)) => Ok(Some(InputMessage::Button(btn, *f != 0.0))),
                other => bail!("input button {suffix:?} expected an int, got {other:?}"),
            };
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_param(value: ParamValue) {
        let m = ParamMessage::new("VRCEmote", value);
        let osc = m.to_osc();
        assert_eq!(osc.addr, "/avatar/parameters/VRCEmote");
        let back = ParamMessage::from_osc(&osc).unwrap().unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn param_bool_roundtrips() {
        roundtrip_param(ParamValue::Bool(true));
        roundtrip_param(ParamValue::Bool(false));
    }

    #[test]
    fn param_int_roundtrips() {
        roundtrip_param(ParamValue::Int(7));
        roundtrip_param(ParamValue::Int(0));
    }

    #[test]
    fn param_float_roundtrips() {
        roundtrip_param(ParamValue::Float(0.5));
        roundtrip_param(ParamValue::Float(-1.0));
    }

    #[test]
    fn param_type_tags() {
        assert_eq!(ParamValue::Bool(true).type_tag(), "b");
        assert_eq!(ParamValue::Int(1).type_tag(), "i");
        assert_eq!(ParamValue::Float(1.0).type_tag(), "f");
    }

    #[test]
    fn non_param_address_is_none() {
        let msg = OscMessage {
            addr: "/input/Jump".to_string(),
            args: vec![OscType::Int(1)],
        };
        assert!(ParamMessage::from_osc(&msg).unwrap().is_none());
    }

    #[test]
    fn empty_param_name_errors() {
        let msg = OscMessage {
            addr: PARAM_PREFIX.to_string(),
            args: vec![OscType::Int(1)],
        };
        assert!(ParamMessage::from_osc(&msg).is_err());
    }

    #[test]
    fn param_missing_arg_errors() {
        let msg = OscMessage {
            addr: "/avatar/parameters/X".to_string(),
            args: vec![],
        };
        assert!(ParamMessage::from_osc(&msg).is_err());
    }

    #[test]
    fn avatar_change_roundtrips_with_path() {
        let c = AvatarChange {
            id: "avtr_1234".to_string(),
            config_path: Some("/home/u/.config/.../avtr_1234.json".to_string()),
        };
        let osc = c.to_osc();
        assert_eq!(osc.addr, "/avatar/change");
        assert_eq!(AvatarChange::from_osc(&osc).unwrap().unwrap(), c);
    }

    #[test]
    fn avatar_change_roundtrips_id_only() {
        let c = AvatarChange {
            id: "avtr_abcd".to_string(),
            config_path: None,
        };
        assert_eq!(AvatarChange::from_osc(&c.to_osc()).unwrap().unwrap(), c);
    }

    #[test]
    fn avatar_change_without_id_errors() {
        let msg = OscMessage {
            addr: AVATAR_CHANGE.to_string(),
            args: vec![OscType::Int(3)],
        };
        assert!(AvatarChange::from_osc(&msg).is_err());
    }

    #[test]
    fn input_axis_roundtrips_and_clamps() {
        let m = InputMessage::Axis(InputAxis::Vertical, 1.5);
        let osc = m.to_osc();
        assert_eq!(osc.addr, "/input/Vertical");
        assert_eq!(osc.args, vec![OscType::Float(1.0)], "clamped to 1.0");
        // Round-trip an in-range value exactly.
        let m2 = InputMessage::Axis(InputAxis::LookHorizontal, -0.5);
        assert_eq!(InputMessage::from_osc(&m2.to_osc()).unwrap().unwrap(), m2);
    }

    #[test]
    fn input_button_roundtrips_both_states() {
        for pressed in [true, false] {
            let m = InputMessage::Button(InputButton::Jump, pressed);
            let osc = m.to_osc();
            assert_eq!(osc.addr, "/input/Jump");
            assert_eq!(InputMessage::from_osc(&osc).unwrap().unwrap(), m);
        }
    }

    #[test]
    fn every_axis_name_roundtrips() {
        for a in [
            InputAxis::Vertical,
            InputAxis::Horizontal,
            InputAxis::LookHorizontal,
            InputAxis::LookVertical,
            InputAxis::MoveHold,
            InputAxis::SpinHold,
            InputAxis::UseAxisRight,
            InputAxis::GrabAxisRight,
            InputAxis::MoveHoldFB,
            InputAxis::SpinHoldCwCcw,
            InputAxis::SpinHoldUD,
            InputAxis::SpinHoldLR,
        ] {
            assert_eq!(InputAxis::from_name(a.name()), Some(a));
        }
    }

    #[test]
    fn every_button_name_roundtrips() {
        for b in [
            InputButton::MoveForward,
            InputButton::MoveBackward,
            InputButton::MoveLeft,
            InputButton::MoveRight,
            InputButton::LookLeft,
            InputButton::LookRight,
            InputButton::Jump,
            InputButton::Run,
            InputButton::ComfortLeft,
            InputButton::ComfortRight,
            InputButton::DropRight,
            InputButton::UseRight,
            InputButton::GrabRight,
            InputButton::DropLeft,
            InputButton::UseLeft,
            InputButton::GrabLeft,
            InputButton::PanicButton,
            InputButton::QuickMenuToggleLeft,
            InputButton::QuickMenuToggleRight,
            InputButton::Voice,
        ] {
            assert_eq!(InputButton::from_name(b.name()), Some(b));
        }
    }

    #[test]
    fn unknown_input_suffix_is_none() {
        let msg = OscMessage {
            addr: "/input/Nonsense".to_string(),
            args: vec![OscType::Int(1)],
        };
        assert!(InputMessage::from_osc(&msg).unwrap().is_none());
    }
}
