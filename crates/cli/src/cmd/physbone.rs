//! `avatar physbone list|set|split|stretch` — inspect and retune the `VRCPhysBone` components of
//! an SDK3 prefab in place (surgical, round-trip-safe edits: only the touched component bodies
//! and bone offsets change; every fileID, reference, and untouched byte survives). Behaviour lives
//! in `avatar_migrate::physbone`; this is the arg surface and the human/JSON report.
//!
//! Writes go through the shared [`WriteGuard`](crate::cmd::WriteGuard): with no `-o` the edited
//! prefab is printed to stdout (a preview); `-o <file>` writes it (the input path with `--force`
//! edits in place); `--dry-run` reports what would change without touching anything.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use avatar_migrate::physbone::{
    self, FlareReport, FlareTarget, PhysBoneInfo, SplitChain, StretchAmount, StretchReport, Tuning,
};
use avatar_migrate::rewrite::PrefabRewriter;
use avatar_migrate::sdk3::{Curve, LimitType};
use clap::{Args, Subcommand};
use serde_json::json;

use crate::cmd::{WriteGuard, write_out_guarded};

#[derive(Subcommand, Debug)]
pub enum PhysBoneCommand {
    /// List every VRCPhysBone in a prefab: root, chains (bones, length), colliders, tuning.
    List(ListArgs),
    /// Retune one PhysBone: values, per-chain curves, ignore list, colliders.
    Set(SetArgs),
    /// Move named chains of a PhysBone onto their own components (tune hair strands apart).
    Split(SplitArgs),
    /// Lengthen a PhysBone's chains by scaling the bone offsets below the hinge (longer skirt/tail).
    Stretch(StretchArgs),
    /// Re-angle a PhysBone's chains toward/away from straight down (a funnel skirt hugs the legs).
    Flare(FlareArgs),
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// The SDK3 prefab (`.prefab`) to inspect.
    prefab: PathBuf,
    /// Print a machine-readable JSON report.
    #[arg(long)]
    json: bool,
}

/// PhysBone parameters shared by `set` and `split`. Every one is optional: unset = keep the
/// component's (or, for `split`, the parent's) value. Curves are `t:v,t:v` keys — position along
/// the chain (0 = first bone, 1 = tip) to a 0..1 multiplier of the base value; pass an empty
/// string to clear a curve.
#[derive(Args, Debug, Clone, Default)]
pub struct TuningArgs {
    /// Pull: how strongly bones return to rest (0..1).
    #[arg(long)]
    pull: Option<f64>,
    /// Pull curve along the chain, e.g. `0:0.6,1:1` (more return at the tips).
    #[arg(long, value_name = "T:V,…")]
    pull_curve: Option<String>,
    /// Spring: bounciness / how much bones overshoot (0..1; lower = more damped).
    #[arg(long)]
    spring: Option<f64>,
    /// Spring curve along the chain, e.g. `0:1,1:0.5` (calmer tips).
    #[arg(long, value_name = "T:V,…")]
    spring_curve: Option<String>,
    /// Stiffness: resistance to bending away from the parent's direction (0..1).
    #[arg(long)]
    stiffness: Option<f64>,
    /// Stiffness curve along the chain.
    #[arg(long, value_name = "T:V,…")]
    stiffness_curve: Option<String>,
    /// Gravity: constant downward pull (0..1); adds "weight".
    #[arg(long)]
    gravity: Option<f64>,
    /// Gravity curve along the chain.
    #[arg(long, value_name = "T:V,…")]
    gravity_curve: Option<String>,
    /// Gravity falloff: how much gravity is cancelled at rest (0 = full gravity always).
    #[arg(long)]
    gravity_falloff: Option<f64>,
    /// Immobile: how much avatar motion is ignored (0..1); `--immobile-type` picks which motion.
    #[arg(long)]
    immobile: Option<f64>,
    /// Immobile curve along the chain.
    #[arg(long, value_name = "T:V,…")]
    immobile_curve: Option<String>,
    /// Immobile type: `all` (All Motion) or `world` (World / parent motion only).
    #[arg(long, value_enum)]
    immobile_type: Option<ImmobileType>,
    /// Collision radius (metres, in the root's scale).
    #[arg(long)]
    radius: Option<f64>,
    /// Radius curve along the chain.
    #[arg(long, value_name = "T:V,…")]
    radius_curve: Option<String>,
    /// Limit type: `none`, `angle`, `hinge`, `polar`.
    #[arg(long, value_enum)]
    limit_type: Option<LimitKind>,
    /// Max angle X (degrees) for angle/hinge/polar limits.
    #[arg(long)]
    max_angle: Option<f64>,
    /// Max-angle curve along the chain.
    #[arg(long, value_name = "T:V,…")]
    max_angle_curve: Option<String>,
    /// Max angle Z (degrees) for polar limits.
    #[arg(long)]
    max_angle_z: Option<f64>,
    /// Integration type: `simplified` or `advanced`.
    #[arg(long, value_enum)]
    integration: Option<Integration>,
    /// PhysBone version: `1.0` or `1.1`.
    #[arg(long, value_name = "1.0|1.1")]
    version: Option<String>,
    /// Multi-child type when the root has several children: `ignore`, `first`, `average`.
    #[arg(long, value_enum)]
    multi_child: Option<MultiChild>,
    /// Allow collision (`0`/`1`).
    #[arg(long, value_name = "0|1")]
    allow_collision: Option<u8>,
    /// Allow grabbing (`0`/`1`).
    #[arg(long, value_name = "0|1")]
    allow_grabbing: Option<u8>,
    /// Allow posing (`0`/`1`).
    #[arg(long, value_name = "0|1")]
    allow_posing: Option<u8>,
    /// Max stretch (0 = none).
    #[arg(long)]
    max_stretch: Option<f64>,
    /// Max squish (0 = none).
    #[arg(long)]
    max_squish: Option<f64>,
    /// Is Animated (`0`/`1`): let animations move the bones.
    #[arg(long, value_name = "0|1")]
    is_animated: Option<u8>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ImmobileType {
    All,
    World,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum LimitKind {
    None,
    Angle,
    Hinge,
    Polar,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum Integration {
    Simplified,
    Advanced,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum MultiChild {
    Ignore,
    First,
    Average,
}

impl TuningArgs {
    fn to_tuning(&self) -> Result<Tuning> {
        let curve = |s: &Option<String>, what: &str| -> Result<Option<Curve>> {
            match s {
                None => Ok(None),
                Some(t) if t.trim().is_empty() => Ok(Some(Curve::NONE)),
                Some(t) => Curve::parse(t)
                    .map(Some)
                    .with_context(|| format!("--{what}-curve")),
            }
        };
        let flag = |v: Option<u8>| v.map(|x| x != 0);
        let version = match self.version.as_deref() {
            None => None,
            Some("1.0") | Some("0") => Some(0),
            Some("1.1") | Some("1") => Some(1),
            Some(other) => anyhow::bail!("--version must be 1.0 or 1.1 (got '{other}')"),
        };
        Ok(Tuning {
            version,
            integration_type: self.integration.map(|i| match i {
                Integration::Simplified => 0,
                Integration::Advanced => 1,
            }),
            multi_child_type: self.multi_child.map(|m| match m {
                MultiChild::Ignore => 0,
                MultiChild::First => 1,
                MultiChild::Average => 2,
            }),
            pull: self.pull,
            pull_curve: curve(&self.pull_curve, "pull")?,
            spring: self.spring,
            spring_curve: curve(&self.spring_curve, "spring")?,
            stiffness: self.stiffness,
            stiffness_curve: curve(&self.stiffness_curve, "stiffness")?,
            gravity: self.gravity,
            gravity_curve: curve(&self.gravity_curve, "gravity")?,
            gravity_falloff: self.gravity_falloff,
            immobile_type: self.immobile_type.map(|t| match t {
                ImmobileType::All => 0,
                ImmobileType::World => 1,
            }),
            immobile: self.immobile,
            immobile_curve: curve(&self.immobile_curve, "immobile")?,
            radius: self.radius,
            radius_curve: curve(&self.radius_curve, "radius")?,
            allow_collision: flag(self.allow_collision),
            limit_type: self.limit_type.map(|l| match l {
                LimitKind::None => LimitType::None,
                LimitKind::Angle => LimitType::Angle,
                LimitKind::Hinge => LimitType::Hinge,
                LimitKind::Polar => LimitType::Polar,
            }),
            max_angle_x: self.max_angle,
            max_angle_x_curve: curve(&self.max_angle_curve, "max-angle")?,
            max_angle_z: self.max_angle_z,
            allow_grabbing: flag(self.allow_grabbing),
            allow_posing: flag(self.allow_posing),
            max_stretch: self.max_stretch,
            max_squish: self.max_squish,
            is_animated: flag(self.is_animated),
        })
    }
}

/// Output/report options shared by the editing subcommands.
#[derive(Args, Debug, Clone)]
pub struct EditOut {
    /// Write the edited prefab here instead of stdout. Pass the input path (with `--force`) to
    /// edit in place.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Print a machine-readable JSON report (before/after state) instead of the prefab text. `-o`
    /// still controls where the prefab is written.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

#[derive(Args, Debug)]
pub struct SetArgs {
    /// The SDK3 prefab (`.prefab`) to edit.
    prefab: PathBuf,
    /// Which PhysBone: its root transform (unique name or `A/B/C` path from the avatar root), the
    /// GameObject carrying it, or its `&fileID`.
    target: String,
    #[command(flatten)]
    tuning: TuningArgs,
    /// Add a transform (name/path, or a child name of the root) to `ignoreTransforms`. Repeatable.
    #[arg(long = "ignore", value_name = "NAME|PATH")]
    ignore_add: Vec<String>,
    /// Remove a transform from `ignoreTransforms`. Repeatable.
    #[arg(long = "unignore", value_name = "NAME|PATH")]
    ignore_remove: Vec<String>,
    /// Add the VRCPhysBoneCollider on this GameObject (name/path) to `colliders`. Repeatable.
    #[arg(long = "collider", value_name = "NAME|PATH")]
    colliders_add: Vec<String>,
    /// Remove a collider (by its GameObject name/path). Repeatable.
    #[arg(long = "uncollider", value_name = "NAME|PATH")]
    colliders_remove: Vec<String>,
    #[command(flatten)]
    out: EditOut,
}

#[derive(Args, Debug)]
pub struct SplitArgs {
    /// The SDK3 prefab (`.prefab`) to edit.
    prefab: PathBuf,
    /// The PhysBone to split (root name/path, GameObject, or `&fileID`).
    target: String,
    /// A chain to move onto its own component: a child of the root (name) or a path under it.
    /// Repeatable; one new PhysBone per chain, rooted on that bone.
    #[arg(long = "chain", value_name = "NAME|PATH", required = true)]
    chains: Vec<String>,
    /// Tuning applied to the new components (over the parent's values).
    #[command(flatten)]
    tuning: TuningArgs,
    #[command(flatten)]
    out: EditOut,
}

#[derive(Args, Debug)]
pub struct StretchArgs {
    /// The SDK3 prefab (`.prefab`) to edit.
    prefab: PathBuf,
    /// The PhysBone whose chains to lengthen (root name/path, GameObject, or `&fileID`).
    target: String,
    /// Length multiplier for the bone offsets (1.5 = 50% longer). Chains with more bones grow more.
    #[arg(long, conflicts_with = "by", required_unless_present = "by")]
    factor: Option<f64>,
    /// Add this length (metres, avatar space) to every chain instead — each chain gets its own
    /// factor, so chains of unequal bone count grow by the same amount and an even hem stays
    /// even. Negative shortens.
    #[arg(long, required_unless_present = "factor", allow_hyphen_values = true)]
    by: Option<f64>,
    /// First depth below the PhysBone root whose offsets are scaled (1 = the root's children, 2 =
    /// grandchildren, …; the root never moves). Default 2 keeps the root's children — a skirt's
    /// hinges — in place; use 1 for a component rooted on a chain's own first bone.
    #[arg(long, default_value_t = 2)]
    from_depth: usize,
    #[command(flatten)]
    out: EditOut,
}

#[derive(Args, Debug)]
pub struct FlareArgs {
    /// The SDK3 prefab (`.prefab`) to edit.
    prefab: PathBuf,
    /// The PhysBone whose chains to re-angle (root name/path, GameObject, or `&fileID`).
    target: String,
    /// New angle from straight down, in degrees, for every chain (0 = hang vertically).
    #[arg(long, conflicts_with = "scale", required_unless_present = "scale")]
    angle: Option<f64>,
    /// Multiply every chain's current angle from straight down (0.5 = half the flare).
    #[arg(long, required_unless_present = "angle")]
    scale: Option<f64>,
    /// Which transform of each chain to rotate: depth below the PhysBone root (1 = the root's
    /// children — a skirt's hinge ring; 0 = the root itself, for a component rooted on the chain's
    /// first bone).
    #[arg(long, default_value_t = 1)]
    hinge_depth: usize,
    #[command(flatten)]
    out: EditOut,
}

fn load(prefab: &Path) -> Result<(String, PrefabRewriter)> {
    let text =
        std::fs::read_to_string(prefab).with_context(|| format!("reading {}", prefab.display()))?;
    let rw = PrefabRewriter::new(&text)
        .with_context(|| format!("parsing {} as a Unity prefab", prefab.display()))?;
    Ok((text, rw))
}

fn print_info(pb: &PhysBoneInfo) {
    println!(
        "PhysBone &{}  on '{}'  root '{}'",
        pb.file_id,
        if pb.object.is_empty() {
            "<avatar root>"
        } else {
            &pb.object
        },
        if pb.root.is_empty() {
            "<avatar root>"
        } else {
            &pb.root
        }
    );
    println!(
        "  version {}  integration {}  multiChild {}  isAnimated {}{}",
        if pb.version == 0 { "1.0" } else { "1.1" },
        if pb.integration_type == 0 {
            "simplified"
        } else {
            "advanced"
        },
        ["ignore", "first", "average"]
            .get(pb.multi_child_type as usize)
            .unwrap_or(&"?"),
        pb.is_animated as u8,
        if pb.parameter.is_empty() {
            String::new()
        } else {
            format!("  parameter '{}'", pb.parameter)
        }
    );
    let curve = |k: &[(f64, f64)]| -> String {
        if k.is_empty() {
            String::new()
        } else {
            format!(" [{}]", Curve(k.to_vec()).describe())
        }
    };
    println!(
        "  pull {}{}  spring {}{}  stiffness {}{}",
        pb.pull,
        curve(&pb.pull_curve),
        pb.spring,
        curve(&pb.spring_curve),
        pb.stiffness,
        curve(&pb.stiffness_curve)
    );
    println!(
        "  gravity {}{} (falloff {})  immobile {}{} ({})",
        pb.gravity,
        curve(&pb.gravity_curve),
        pb.gravity_falloff,
        pb.immobile,
        curve(&pb.immobile_curve),
        if pb.immobile_type == 0 {
            "all motion"
        } else {
            "world motion"
        }
    );
    println!(
        "  limit {}  maxAngle {}{}{}  radius {}{}  collision {}  grab {}  pose {}  stretch {}  squish {}",
        ["none", "angle", "hinge", "polar"]
            .get(pb.limit_type as usize)
            .unwrap_or(&"?"),
        pb.max_angle_x,
        curve(&pb.max_angle_x_curve),
        if pb.limit_type == 3 {
            format!("/{}", pb.max_angle_z)
        } else {
            String::new()
        },
        pb.radius,
        curve(&pb.radius_curve),
        pb.allow_collision as u8,
        pb.allow_grabbing as u8,
        pb.allow_posing as u8,
        pb.max_stretch,
        pb.max_squish
    );
    if !pb.ignore.is_empty() {
        println!("  ignore: {}", pb.ignore.join(", "));
    }
    if !pb.colliders.is_empty() {
        println!("  colliders: {}", pb.colliders.join(", "));
    }
    println!(
        "  {} transform(s) in {} chain(s):",
        pb.transforms,
        pb.chains.len()
    );
    for c in &pb.chains {
        println!(
            "    {} — {} bone(s), {:.3} m, {:.1}° from down",
            c.leaf, c.bones, c.length, c.flare_deg
        );
    }
}

pub fn list(args: &ListArgs) -> Result<()> {
    let (_, rw) = load(&args.prefab)?;
    let all = physbone::list(rw.scene());
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "prefab": args.prefab,
                "physbones": all,
            }))?
        );
        return Ok(());
    }
    if all.is_empty() {
        println!("no VRCPhysBone components in {}", args.prefab.display());
        return Ok(());
    }
    println!(
        "{}: {} VRCPhysBone component(s)",
        args.prefab.display(),
        all.len()
    );
    for pb in &all {
        println!();
        print_info(pb);
    }
    Ok(())
}

/// Emit the edited prefab per the guard, and the report per `--json`. The file is written
/// before anything is printed, so a closed stdout (`| head`) can never lose the edit.
fn finish(
    prefab: &Path,
    rw: PrefabRewriter,
    out: &EditOut,
    report: serde_json::Value,
    human: impl FnOnce(),
) -> Result<()> {
    let text = rw.into_string();
    if let Some(o) = &out.output {
        write_out_guarded(Some(o), &text, out.guard)?;
    }
    if out.json {
        let mut r = report;
        r["prefab"] = json!(prefab);
        r["output"] = json!(out.output);
        r["dry_run"] = json!(out.guard.dry_run);
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else if out.output.is_some() {
        human();
    } else {
        // No file target: the prefab itself goes to stdout (a preview), nothing else.
        write_out_guarded(None, &text, out.guard)?;
    }
    Ok(())
}

pub fn set(args: &SetArgs) -> Result<()> {
    let (_, mut rw) = load(&args.prefab)?;
    let id = physbone::find(rw.scene(), &args.target)?;
    let before = physbone::info(rw.scene(), id)?;
    let tuning = args.tuning.to_tuning()?;
    if tuning.is_empty()
        && args.ignore_add.is_empty()
        && args.ignore_remove.is_empty()
        && args.colliders_add.is_empty()
        && args.colliders_remove.is_empty()
    {
        anyhow::bail!(
            "nothing to set — pass at least one tuning flag, --ignore/--unignore, or --collider/--uncollider"
        );
    }
    let after = physbone::set(
        &mut rw,
        id,
        &tuning,
        &args.ignore_add,
        &args.ignore_remove,
        &args.colliders_add,
        &args.colliders_remove,
    )?;
    let changes = tuning.changes();
    let report = json!({
        "physbone": id,
        "changes": changes,
        "ignore_added": args.ignore_add,
        "ignore_removed": args.ignore_remove,
        "colliders_added": args.colliders_add,
        "colliders_removed": args.colliders_remove,
        "before": before,
        "after": after,
    });
    finish(&args.prefab, rw, &args.out, report, || {
        println!(
            "set on PhysBone &{id} ('{}'): {}",
            after.root,
            changes.join(", ")
        );
        print_info(&after);
    })
}

pub fn split(args: &SplitArgs) -> Result<()> {
    let (_, mut rw) = load(&args.prefab)?;
    let id = physbone::find(rw.scene(), &args.target)?;
    let tuning = args.tuning.to_tuning()?;
    let moved: Vec<SplitChain> = physbone::split(&mut rw, id, &args.chains, &tuning)?;
    // Re-parse for the after-state of every component involved.
    let after = PrefabRewriter::new(rw.text())?;
    let parent = physbone::info(after.scene(), id)?;
    let children: Vec<PhysBoneInfo> = moved
        .iter()
        .map(|m| physbone::info(after.scene(), m.file_id))
        .collect::<Result<_>>()?;
    let report = json!({
        "physbone": id,
        "split": moved,
        "tuning": tuning.changes(),
        "parent": parent,
        "children": children,
    });
    finish(&args.prefab, rw, &args.out, report, || {
        println!(
            "split {} chain(s) off PhysBone &{id} ('{}'):",
            moved.len(),
            parent.root
        );
        for m in &moved {
            println!(
                "  {} -> new PhysBone &{} ({} bone(s), {:.3} m)",
                m.path, m.file_id, m.bones, m.length
            );
        }
        println!();
        print_info(&parent);
        for c in &children {
            println!();
            print_info(c);
        }
    })
}

pub fn stretch(args: &StretchArgs) -> Result<()> {
    let (_, mut rw) = load(&args.prefab)?;
    let id = physbone::find(rw.scene(), &args.target)?;
    let amount = match (args.factor, args.by) {
        (Some(f), _) => StretchAmount::Factor(f),
        (None, Some(b)) => StretchAmount::By(b),
        (None, None) => anyhow::bail!("pass --factor F or --by METERS"),
    };
    let r: StretchReport = physbone::stretch_with(&mut rw, id, amount, args.from_depth)?;
    let report = json!({
        "physbone": id,
        "stretch": r,
    });
    finish(&args.prefab, rw, &args.out, report, || {
        println!(
            "stretched PhysBone &{id} chains {} ({} bone offset(s) scaled, from depth {}):",
            match r.by {
                Some(b) => format!("by {b:+} m each"),
                None => format!("by x{}", r.factor),
            },
            r.bones.len(),
            args.from_depth
        );
        for (leaf, b, a) in &r.chains {
            println!("  {leaf}: {b:.3} m -> {a:.3} m");
        }
    })
}

pub fn flare(args: &FlareArgs) -> Result<()> {
    let (_, mut rw) = load(&args.prefab)?;
    let id = physbone::find(rw.scene(), &args.target)?;
    let target = match (args.angle, args.scale) {
        (Some(a), _) => FlareTarget::Angle(a),
        (None, Some(s)) => FlareTarget::Scale(s),
        (None, None) => anyhow::bail!("pass --angle DEG or --scale FACTOR"),
    };
    let r: FlareReport = physbone::flare(&mut rw, id, target, args.hinge_depth)?;
    let report = json!({
        "physbone": id,
        "flare": r,
    });
    finish(&args.prefab, rw, &args.out, report, || {
        println!(
            "re-angled {} chain(s) of PhysBone &{id} ({}):",
            r.chains.len(),
            match target {
                FlareTarget::Angle(a) => format!("to {a}° from down"),
                FlareTarget::Scale(s) => format!("angle from down x{s}"),
            }
        );
        for c in &r.chains {
            println!("  {}: {:.1}° -> {:.1}°", c.hinge, c.before_deg, c.after_deg);
        }
    })
}
