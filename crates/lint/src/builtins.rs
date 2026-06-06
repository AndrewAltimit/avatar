//! VRChat's built-in avatar parameters. These are provided by the platform and do not need to
//! be declared in an Expression Parameters asset, so menu references to them are not flagged.
//!
//! Source: <https://creators.vrchat.com/avatars/animator-parameters/> (built-in inputs).

pub const BUILTIN_PARAMETERS: &[&str] = &[
    "IsLocal",
    "PreviewMode",
    "Viseme",
    "Voice",
    "GestureLeft",
    "GestureRight",
    "GestureLeftWeight",
    "GestureRightWeight",
    "AngularY",
    "VelocityX",
    "VelocityY",
    "VelocityZ",
    "VelocityMagnitude",
    "Upright",
    "Grounded",
    "Seated",
    "AFK",
    "TrackingType",
    "VRMode",
    "MuteSelf",
    "InStation",
    "Earmuffs",
    "IsOnFriendsList",
    "AvatarVersion",
    "ScaleModified",
    "ScaleFactor",
    "ScaleFactorInverse",
    "EyeHeightAsMeters",
    "EyeHeightAsPercent",
];
