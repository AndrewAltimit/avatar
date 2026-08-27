//! `avatar-gesture-capture` — standalone capture session for VRChat's OSC parameter output,
//! reduced to the gesture cross-tab report. The single-file, no-argument-parser sibling of
//! `avatar osc capture`, kept dependency-slim so it cross-compiles to a drop-on-the-desktop
//! Windows `.exe`: enable OSC in VRChat (Action Menu → Options → OSC), run it, sweep the
//! touchpad and trigger for a minute, read the table.
//!
//! Usage: `avatar-gesture-capture [seconds] [port]` (defaults: 60, 9001). Raw events are
//! appended to `gesture-capture.jsonl` next to the executable.

use std::io::Write as _;
use std::time::{Duration, Instant};

use avatar_osc::ParamClient;
use avatar_osc::capture::{Capture, render_summary};
use avatar_osc::oscquery::OscQueryAdvertiser;

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(60);
    let port: u16 = args.next().and_then(|a| a.parse().ok()).unwrap_or(9001);

    // Bind the requested port, or fall back to an ephemeral one — the OSCQuery advertisement
    // below tells VRChat where to send either way.
    let mut client = match ParamClient::new(("127.0.0.1", port), ("127.0.0.1", 9000))
        .or_else(|_| ParamClient::new(("127.0.0.1", 0), ("127.0.0.1", 9000)))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: could not bind a listen socket: {e:#}");
            wait_for_enter();
            std::process::exit(1);
        }
    };
    let listen_port = client.local_addr().map(|a| a.port()).unwrap_or(port);
    // Advertise over OSCQuery/mDNS so modern VRChat discovers us and routes its parameter
    // output here — no fixed-port assumptions. (Windows Firewall may ask once; allow it.)
    let _advertiser = match OscQueryAdvertiser::start("avatar-capture", listen_port) {
        Ok(a) => {
            println!(
                "advertising OSCQuery service '{}' (HTTP :{}) -> OSC :{listen_port}",
                a.name(),
                a.http_port()
            );
            Some(a)
        }
        Err(e) => {
            println!("note: OSCQuery advertisement failed ({e:#}); legacy port {listen_port} only");
            None
        }
    };
    let log_path = "gesture-capture.jsonl";
    let mut sink = std::fs::File::create(log_path)
        .ok()
        .map(std::io::BufWriter::new);

    println!("capturing VRChat parameters on 127.0.0.1:{listen_port} for {seconds}s…");
    println!("make sure OSC is enabled (Action Menu -> Options -> OSC) — toggling it off/on");
    println!("forces re-discovery — then sweep every touchpad region with and without the");
    println!("trigger, both hands.\n");

    let mut cap = Capture::new();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    let mut nagged = false;
    let mut vrchat_seen = false;
    while Instant::now() < deadline {
        if let Some(a) = &_advertiser {
            for inst in a.vrchat_announcements() {
                if !vrchat_seen {
                    println!("(VRChat's own OSCQuery service is announcing: {inst})");
                    vrchat_seen = true;
                }
            }
        }
        if !nagged && cap.events().is_empty() && start.elapsed() > Duration::from_secs(10) {
            nagged = true;
            println!("no parameters after 10s — is OSC enabled and an avatar loaded? Try");
            println!("toggling OSC off/on in the Action Menu to force re-discovery.");
        }
        match client.poll() {
            Ok(updates) => {
                for update in updates {
                    let e = cap.record(&update.name, update.value);
                    if let Some(sink) = &mut sink {
                        let _ = serde_json::to_writer(&mut *sink, e);
                        let _ = sink.write_all(b"\n");
                    }
                    println!("{:8.3}s {} = {:?}", e.t, e.name, update.value);
                }
            }
            Err(e) => eprintln!("recv error: {e:#}"),
        }
        if let Some(sink) = &mut sink {
            let _ = sink.flush();
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    println!("\n{}", render_summary(&cap.summary()));
    println!("raw events: {log_path} — send this file back for analysis.");
    wait_for_enter();
}

/// Keep the console window open when double-clicked on Windows.
fn wait_for_enter() {
    println!("\npress Enter to close…");
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}
