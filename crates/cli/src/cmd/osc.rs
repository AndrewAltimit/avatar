//! `avatar osc` — drive or observe a running VRChat avatar over OSC, and the analog-gesture daemon.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use avatar_osc::{AvatarConfig, InputAxis, InputButton, ParamClient, ParamValue};
use clap::{Args, Subcommand};

use crate::cmd::{parse_bool, parse_bool_opt};

#[derive(Subcommand, Debug)]
pub enum OscCommand {
    /// Set one avatar parameter: `/avatar/parameters/<NAME>`.
    Send(OscSendArgs),
    /// Send an input axis (`-1..1`) or button (`true`/`false`): `/input/<NAME>`.
    Input(OscInputArgs),
    /// Listen for the avatar parameters VRChat broadcasts and print each update.
    Monitor(OscMonitorArgs),
    /// Request VRChat switch avatars: `/avatar/change <blueprint-id>`.
    Change(OscChangeArgs),
    /// Parse an avatar's OSCQuery config JSON and list its parameters (offline; no socket).
    Query(OscQueryArgs),
    /// Run the analog-gesture daemon: map a controller trigger → `Gesture*`/`Gesture*Weight` over
    /// OSC. With no on-device input backend headless, this drives a synthetic demo trigger sweep.
    Gestures(OscGesturesArgs),
}

/// Where a running VRChat listens for our messages (its default OSC-in port is 9000).
#[derive(Args, Debug)]
pub struct OscTarget {
    /// Host VRChat is reachable at.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Port VRChat listens on for incoming OSC.
    #[arg(long, default_value_t = avatar_osc::VRCHAT_RECV_PORT)]
    port: u16,
}

#[derive(Args, Debug)]
pub struct OscSendArgs {
    /// Parameter name (without the `/avatar/parameters/` prefix).
    name: String,
    /// Value: `true`/`false` (bool), an integer (int), or anything else parses as float.
    /// Override the auto-detected type with `--type`.
    value: String,
    /// Force the OSC value type instead of auto-detecting from `value`.
    #[arg(long = "type", value_enum)]
    value_type: Option<OscValueType>,
    #[command(flatten)]
    target: OscTarget,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum OscValueType {
    Bool,
    Int,
    Float,
}

#[derive(Args, Debug)]
pub struct OscInputArgs {
    /// Input name — a VRChat axis (e.g. `Vertical`) or button (e.g. `Jump`).
    name: String,
    /// Axis value (`-1..1`) or button state (`true`/`false`), matching the input's kind.
    value: String,
    #[command(flatten)]
    target: OscTarget,
}

#[derive(Args, Debug)]
pub struct OscChangeArgs {
    /// Avatar blueprint id (`avtr_…`).
    id: String,
    #[command(flatten)]
    target: OscTarget,
}

#[derive(Args, Debug)]
pub struct OscMonitorArgs {
    /// Address to listen on (VRChat's default OSC-out port is 9001).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Port to listen on for VRChat's outgoing parameters.
    #[arg(long, default_value_t = avatar_osc::VRCHAT_SEND_PORT)]
    port: u16,
    /// Stop after this many seconds. Without it, runs until interrupted (Ctrl-C).
    #[arg(long)]
    seconds: Option<u64>,
}

#[derive(Args, Debug)]
pub struct OscQueryArgs {
    /// Path to an avatar's OSCQuery config JSON (VRChat writes these under its OSC/ folder).
    path: PathBuf,
    /// Emit the parsed parameter list as JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct OscGesturesArgs {
    /// Tick rate in Hz (how often the daemon polls input and sends OSC).
    #[arg(long, default_value_t = 100)]
    hz: u32,
    /// Demo trigger-sweep period, in ticks (a full 0→1→0 pull). Default ~1.2s at 100 Hz.
    #[arg(long, default_value_t = 120)]
    period: u64,
    /// Stop after this many seconds. Without it, runs until interrupted (Ctrl-C).
    #[arg(long)]
    seconds: Option<u64>,
    #[command(flatten)]
    target: OscTarget,
}

/// Bind a send-only client (ephemeral local port) aimed at a running VRChat.
fn sender_client(target: &OscTarget) -> Result<ParamClient> {
    ParamClient::new(("0.0.0.0", 0), (target.host.as_str(), target.port))
        .context("opening OSC send socket")
}

/// Send one avatar parameter to VRChat.
pub fn send(args: &OscSendArgs) -> Result<()> {
    let value = parse_param_value(&args.value, args.value_type)?;
    let client = sender_client(&args.target)?;
    client.send_param(&args.name, value)?;
    println!(
        "sent /avatar/parameters/{} = {value:?} -> {}",
        args.name,
        client.target()
    );
    Ok(())
}

/// Resolve a textual value to a [`ParamValue`], honoring an explicit `--type` or auto-detecting
/// `true`/`false` → bool, integer → int, else float.
fn parse_param_value(s: &str, forced: Option<OscValueType>) -> Result<ParamValue> {
    match forced {
        Some(OscValueType::Bool) => Ok(ParamValue::Bool(parse_bool(s)?)),
        Some(OscValueType::Int) => Ok(ParamValue::Int(
            s.parse()
                .with_context(|| format!("'{s}' is not an integer"))?,
        )),
        Some(OscValueType::Float) => Ok(ParamValue::Float(
            s.parse()
                .with_context(|| format!("'{s}' is not a number"))?,
        )),
        None => {
            if let Some(b) = parse_bool_opt(s) {
                Ok(ParamValue::Bool(b))
            } else if let Ok(i) = s.parse::<i32>() {
                Ok(ParamValue::Int(i))
            } else if let Ok(f) = s.parse::<f32>() {
                Ok(ParamValue::Float(f))
            } else {
                bail!("could not parse '{s}' as bool/int/float; pass --type to force a type")
            }
        }
    }
}

/// Send an `/input/<NAME>` axis or button, picking the kind from the canonical name.
pub fn input(args: &OscInputArgs) -> Result<()> {
    let client = sender_client(&args.target)?;
    if let Some(axis) = InputAxis::from_name(&args.name) {
        let v: f32 = args
            .value
            .parse()
            .with_context(|| format!("axis value '{}' must be a float in -1..1", args.value))?;
        client.send_axis(axis, v)?;
        println!("sent /input/{} = {v}", axis.name());
    } else if let Some(button) = InputButton::from_name(&args.name) {
        let pressed = parse_bool(&args.value)?;
        client.send_button(button, pressed)?;
        println!("sent /input/{} = {pressed}", button.name());
    } else {
        bail!(
            "'{}' is not a known VRChat input axis or button (e.g. Vertical, Horizontal, Jump, Voice)",
            args.name
        );
    }
    Ok(())
}

/// Ask VRChat to load a different avatar.
pub fn change(args: &OscChangeArgs) -> Result<()> {
    let client = sender_client(&args.target)?;
    client.send_avatar_change(&args.id)?;
    println!("sent /avatar/change {}", args.id);
    Ok(())
}

/// Listen for the avatar parameters VRChat broadcasts and print each update until the optional
/// time budget elapses (or forever, until Ctrl-C).
pub fn monitor(args: &OscMonitorArgs) -> Result<()> {
    // We only receive here; the send target is unused but the constructor needs one.
    let mut client = ParamClient::new(
        (args.host.as_str(), args.port),
        ("127.0.0.1", avatar_osc::VRCHAT_RECV_PORT),
    )
    .context("binding OSC monitor socket")?;

    match args.seconds {
        Some(s) => println!(
            "listening for VRChat parameters on {}:{} for {s}s…",
            args.host, args.port
        ),
        None => println!(
            "listening for VRChat parameters on {}:{} (Ctrl-C to stop)…",
            args.host, args.port
        ),
    }

    let deadline = args
        .seconds
        .map(|s| Instant::now() + Duration::from_secs(s));
    loop {
        for update in client.poll()? {
            println!("{} = {:?}", update.name, update.value);
        }
        if let Some(d) = deadline
            && Instant::now() >= d
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Parse an avatar's OSCQuery config JSON and list its `/avatar/parameters/*` entries.
pub fn query(args: &OscQueryArgs) -> Result<()> {
    let config = AvatarConfig::from_path(&args.path)?;
    let params: Vec<_> = config.avatar_parameters().collect();

    if args.json {
        let view: Vec<_> = params
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "full_path": p.full_path,
                    "type": p.type_tag,
                    "readable": p.access.is_readable(),
                    "writable": p.access.is_writable(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "name": config.name,
                "parameters": view,
            }))?
        );
        return Ok(());
    }

    println!("OSCQuery config: {}", config.name);
    println!("  {} avatar parameter(s):\n", params.len());
    let (h_name, h_type, h_access) = ("Parameter", "Type", "Access");
    println!("  {h_name:<36} {h_type:<6} {h_access}");
    println!("  {} {} {}", "-".repeat(36), "-".repeat(6), "-".repeat(9));
    for p in &params {
        let name = p.name.as_deref().unwrap_or(&p.full_path);
        let access = access_label(p.access);
        println!("  {name:<36} {:<6} {access}", p.type_tag);
    }
    Ok(())
}

/// Run the analog-gesture daemon. There is no on-device analog input backend headless yet (OpenXR
/// is the production path), so this drives a deterministic synthetic trigger sweep — pull it up in a
/// running VRChat and watch the Fist gesture blend, end-to-end proof of the pipeline.
pub fn gestures(args: &OscGesturesArgs) -> Result<()> {
    use avatar_osc_gestures::{DemoSource, GestureDaemon};

    if args.hz == 0 {
        bail!("--hz must be at least 1");
    }
    let client = sender_client(&args.target)?;
    let mut daemon = GestureDaemon::new(DemoSource::new(args.period));
    let period = Duration::from_secs_f64(1.0 / args.hz as f64);

    println!(
        "gesture daemon (demo): synthesizing a trigger sweep at {} Hz -> {} \
         (real input backend: OpenXR, pending)",
        args.hz,
        client.target()
    );
    match args.seconds {
        Some(s) => {
            println!("running for {s}s…");
            daemon.run_for(&client, period, Duration::from_secs(s))?;
            println!("done.");
            Ok(())
        }
        None => {
            println!("running until interrupted (Ctrl-C)…");
            daemon.run(&client, period)
        }
    }
}

/// A compact label for an OSCQuery access mode.
fn access_label(access: avatar_osc::Access) -> &'static str {
    match (access.is_readable(), access.is_writable()) {
        (true, true) => "read/write",
        (true, false) => "read",
        (false, true) => "write",
        (false, false) => "none",
    }
}
