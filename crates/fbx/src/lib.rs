//! Low-level reader for binary FBX files, tailored to what avatar tooling needs:
//! the object table (`Model`, `Geometry`, ...) and the connection graph that wires
//! objects into a hierarchy. Built on top of `fbxcel`'s node tree.
//!
//! Scope: **binary FBX 7.x only** (the Autodesk / Unity / Blender default). ASCII FBX is
//! not supported by `fbxcel` and is rejected with a clear error.
//!
//! This crate intentionally stays close to the raw FBX structure — it does not compute
//! world transforms or interpret skinning. Higher-level interpretation (skeletons, Unity
//! humanoid mapping) lives in `avatar-armature`.

mod mesh;

use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;

use anyhow::{Context, Result, bail};
use fbxcel::low::FbxVersion;
use fbxcel::low::v7400::AttributeValue;
use fbxcel::tree::any::AnyTree;
use fbxcel::tree::v7400::{NodeHandle, NodeId, Tree};
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

/// A connection between two FBX objects. FBX connections point child -> parent.
#[derive(Debug, Clone)]
pub struct Connection {
    /// Connection type: `"OO"` (object-object) or `"OP"` (object-property), etc.
    pub kind: String,
    /// Source object id (the child).
    pub child: i64,
    /// Destination object id (the parent). `0` is the implicit scene root.
    pub parent: i64,
    /// For `"OP"` connections, the destination property name.
    pub property: Option<String>,
}

/// Local transform components read from a `Model`'s `Properties70`, when present.
/// These are *local* values straight from the file — no parent composition is applied.
#[derive(Debug, Clone, Default)]
pub struct LocalTransform {
    pub translation: Option<[f64; 3]>,
    pub rotation: Option<[f64; 3]>,
    pub scaling: Option<[f64; 3]>,
}

/// A single FBX object (a node under `Objects`).
#[derive(Debug, Clone)]
pub struct FbxObject {
    /// Object id (the first node attribute).
    pub id: i64,
    /// The FBX node name, e.g. `"Model"`, `"Geometry"`, `"Material"`.
    pub node_name: String,
    /// The object's own name (the part before the `\0\1` separator in the name attribute).
    pub name: String,
    /// The object class (the part after the `\0\1` separator), e.g. `"Model"`, `"Geometry"`.
    pub class: String,
    /// The sub-class / third attribute, e.g. `"LimbNode"`, `"Mesh"`, `"Null"`, `"Root"`.
    pub subclass: String,
    /// Local transform components, for `Model` objects that carry them.
    pub transform: LocalTransform,
}

impl FbxObject {
    /// True if this is a `Model` object (something placed in the scene hierarchy).
    pub fn is_model(&self) -> bool {
        self.node_name == "Model"
    }

    /// True if this model looks like a skeleton bone (`LimbNode`, or a `Null`/`Root`
    /// commonly used as a skeleton root).
    pub fn is_bone_like(&self) -> bool {
        self.is_model() && matches!(self.subclass.as_str(), "LimbNode" | "Limb" | "Root")
    }
}

/// Global document settings, useful for diagnosing unit/orientation problems.
#[derive(Debug, Clone, Default)]
pub struct GlobalSettings {
    /// `UnitScaleFactor` — Unity expects FBX authored in centimeters (100.0) for 1:1 scale.
    pub unit_scale_factor: Option<f64>,
    /// `UpAxis` (0 = X, 1 = Y, 2 = Z). Unity expects Y-up (1).
    pub up_axis: Option<i32>,
    /// `FrontAxis`.
    pub front_axis: Option<i32>,
}

/// A parsed FBX scene: the object table plus the connection graph.
#[derive(Debug, Clone)]
pub struct FbxScene {
    /// FBX format version as reported by the parser, e.g. `7400`.
    pub version: u32,
    pub global_settings: GlobalSettings,
    pub objects: Vec<FbxObject>,
    pub connections: Vec<Connection>,
}

impl FbxScene {
    /// Load and parse a binary FBX file.
    pub fn load(path: &Path) -> Result<Self> {
        let (version, tree) = load_tree(path)?;
        Ok(Self::from_tree_ref(&tree, fbx_version_to_u32(version)))
    }

    /// Build the flattened read view (objects, connections, settings) from an already-parsed tree.
    fn from_tree_ref(tree: &Tree, version: u32) -> Self {
        let root = tree.root();
        FbxScene {
            version,
            global_settings: read_global_settings(&root),
            objects: read_objects(&root),
            connections: read_connections(&root),
        }
    }

    /// All `Model` objects.
    pub fn models(&self) -> impl Iterator<Item = &FbxObject> {
        self.objects.iter().filter(|o| o.is_model())
    }

    /// Look up an object by id.
    pub fn object(&self, id: i64) -> Option<&FbxObject> {
        self.objects.iter().find(|o| o.id == id)
    }

    /// Object-object child ids of the given object id, in file order.
    pub fn children_of(&self, parent: i64) -> Vec<i64> {
        self.connections
            .iter()
            .filter(|c| c.kind == "OO" && c.parent == parent)
            .map(|c| c.child)
            .collect()
    }

    /// The parent object id for the given child via an object-object connection, if any.
    pub fn parent_of(&self, child: i64) -> Option<i64> {
        self.connections
            .iter()
            .find(|c| c.kind == "OO" && c.child == child)
            .map(|c| c.parent)
    }
}

/// Maximum FBX node nesting depth we will accept. Real avatar FBX hierarchies are shallow (a
/// `Model` armature a few dozen bones deep, geometry/deformer trees of a handful of levels); this
/// cap is far above any legitimate file and exists only so a pathologically/adversarially nested
/// file bails with a clean error instead of degrading into unbounded memory use during traversal.
const MAX_NODE_DEPTH: usize = 1024;

/// Parse a seekable reader into a versioned node tree.
fn parse_tree<R: std::io::Read + std::io::Seek>(
    reader: R,
    src: &str,
) -> Result<(FbxVersion, Tree)> {
    match AnyTree::from_seekable_reader(reader).with_context(|| format!("parsing FBX {src}"))? {
        AnyTree::V7400(version, tree, _footer) => {
            check_depth(&tree, src)?;
            Ok((version, tree))
        }
        _ => bail!(
            "unsupported FBX tree version; only FBX 7.x binary files are supported \
             (re-export as binary FBX if this is an ASCII file)"
        ),
    }
}

/// Walk the node tree iteratively (no recursion — an adversarial file could be nested far deeper
/// than the native stack tolerates) and bail if it exceeds [`MAX_NODE_DEPTH`]. fbxcel builds the
/// tree with a heap stack, so this is the point at which we reject pathological nesting cleanly.
fn check_depth(tree: &Tree, src: &str) -> Result<()> {
    // Explicit (node, depth) work stack on the heap.
    let mut stack: Vec<(NodeHandle, usize)> = vec![(tree.root(), 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_NODE_DEPTH {
            bail!(
                "FBX {src} exceeds the maximum supported node nesting depth ({MAX_NODE_DEPTH}); \
                 refusing to process a pathologically nested file"
            );
        }
        for child in node.children() {
            stack.push((child, depth + 1));
        }
    }
    Ok(())
}

/// Load and parse a binary FBX file into a versioned node tree.
fn load_tree(path: &Path) -> Result<(FbxVersion, Tree)> {
    let file = File::open(path).with_context(|| format!("opening FBX file {}", path.display()))?;
    parse_tree(BufReader::new(file), &path.display().to_string())
}

/// Collapse an `FbxVersion` to the `major*1000 + minor*100` form used by the read view.
fn fbx_version_to_u32(version: FbxVersion) -> u32 {
    let (major, minor) = version.major_minor();
    major * 1000 + minor * 100
}

/// A loaded FBX document that **retains the mutable node tree**, enabling in-place edits and
/// write-back to a binary FBX. The flattened [`FbxScene`] read view is recomputed on demand via
/// [`FbxDocument::scene`]; diagnosis/planning runs off that, while repairs mutate the tree here.
///
/// Mutators address objects by their FBX object **id** (the stable identifier the connection graph,
/// skin clusters, and animation curves all reference), so e.g. [`rename_object`] never breaks
/// skinning or animation — only the human-facing `Model` name changes.
///
/// [`rename_object`]: FbxDocument::rename_object
#[derive(Debug, Clone)]
pub struct FbxDocument {
    version: FbxVersion,
    tree: Tree,
}

impl FbxDocument {
    /// Load and parse a binary FBX file, retaining its tree for editing.
    pub fn load(path: &Path) -> Result<Self> {
        let (version, tree) = load_tree(path)?;
        Ok(Self { version, tree })
    }

    /// Parse a binary FBX document from an in-memory byte buffer.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let (version, tree) = parse_tree(Cursor::new(bytes), "<bytes>")?;
        Ok(Self { version, tree })
    }

    /// The flattened read view (objects, connections, global settings), recomputed from the
    /// current tree — reflects any edits applied so far.
    pub fn scene(&self) -> FbxScene {
        FbxScene::from_tree_ref(&self.tree, fbx_version_to_u32(self.version))
    }

    /// Extract every mesh's geometry + skin binding as renderer-agnostic [`avatar_mesh::RawMesh`]s.
    ///
    /// Returns triangulated positions (with normals/UVs when the layout is understood) and, for
    /// skinned meshes, the per-bone influence indices/weights and bind matrices needed to pose and
    /// skin the avatar at runtime. Empty if the file has no `Geometry` nodes. Reads array payloads
    /// from the retained tree (they are not on the flattened [`FbxScene`]).
    pub fn meshes(&self) -> Result<Vec<avatar_mesh::RawMesh>> {
        mesh::extract_meshes(&self.tree, &self.scene())
    }

    /// `NodeId` of the object node (e.g. a `Model`) whose object id is `id`.
    fn object_node_id(&self, id: i64) -> Option<NodeId> {
        let root = self.tree.root();
        let objects = child_named(&root, "Objects")?;
        objects
            .children()
            .find(|n| n.attributes().first().and_then(as_i64) == Some(id))
            .map(|n| n.node_id())
    }

    /// Rename an object, rewriting its `Name\0\1Class` attribute while preserving the class suffix.
    /// Connection/skin/animation references are by id, so this is safe within the file.
    pub fn rename_object(&mut self, id: i64, new_name: &str) -> Result<()> {
        let node = self
            .object_node_id(id)
            .with_context(|| format!("FBX object id {id} not found for rename"))?;

        // Read the existing class suffix, then write the recombined name (two non-overlapping borrows).
        let class = match self.tree.get_attribute_mut(node, 1) {
            Some(AttributeValue::String(s)) => split_name_class(s).1,
            _ => bail!("FBX object id {id} has no string name attribute to rename"),
        };
        let combined = if class.is_empty() {
            new_name.to_string()
        } else {
            format!("{new_name}\u{0}\u{1}{class}")
        };
        if let Some(AttributeValue::String(s)) = self.tree.get_attribute_mut(node, 1) {
            *s = combined;
        }
        Ok(())
    }

    /// Re-point an object's object-object (`OO`) parent connection to `new_parent_id`.
    pub fn reparent_object(&mut self, child_id: i64, new_parent_id: i64) -> Result<()> {
        let c_node = {
            let root = self.tree.root();
            let conns = child_named(&root, "Connections").context("FBX has no Connections node")?;
            conns
                .children()
                .find(|n| {
                    n.name() == "C"
                        && n.attributes().first().and_then(as_str) == Some("OO")
                        && n.attributes().get(1).and_then(as_i64) == Some(child_id)
                })
                .map(|n| n.node_id())
        }
        .with_context(|| format!("no OO connection found for child object id {child_id}"))?;

        match self.tree.get_attribute_mut(c_node, 2) {
            Some(attr) => *attr = AttributeValue::I64(new_parent_id),
            None => bail!("connection for child id {child_id} has no parent attribute"),
        }
        Ok(())
    }

    /// `NodeId` of a `GlobalSettings/Properties70/P` node with the given key.
    fn global_setting_node(&self, key: &str) -> Option<NodeId> {
        let root = self.tree.root();
        let gs = child_named(&root, "GlobalSettings")?;
        let props = child_named(&gs, "Properties70")?;
        props
            .children()
            .find(|p| p.name() == "P" && p.attributes().first().and_then(as_str) == Some(key))
            .map(|n| n.node_id())
    }

    /// Set a floating-point global setting (e.g. `UnitScaleFactor`) at the property value slot.
    pub fn set_global_setting_f64(&mut self, key: &str, value: f64) -> Result<()> {
        let node = self
            .global_setting_node(key)
            .with_context(|| format!("GlobalSettings property {key} not found"))?;
        match self.tree.get_attribute_mut(node, 4) {
            Some(attr) => *attr = AttributeValue::F64(value),
            None => bail!("GlobalSettings {key} has no value attribute"),
        }
        Ok(())
    }

    /// Set an integer global setting (e.g. `UpAxis`) at the property value slot.
    pub fn set_global_setting_i32(&mut self, key: &str, value: i32) -> Result<()> {
        let node = self
            .global_setting_node(key)
            .with_context(|| format!("GlobalSettings property {key} not found"))?;
        match self.tree.get_attribute_mut(node, 4) {
            Some(attr) => *attr = AttributeValue::I32(value),
            None => bail!("GlobalSettings {key} has no value attribute"),
        }
        Ok(())
    }

    /// Multiply a model's `Lcl Scaling` (x, y, z) by `factor` in place.
    pub fn scale_object(&mut self, id: i64, factor: f64) -> Result<()> {
        let node = self
            .object_node_id(id)
            .with_context(|| format!("FBX object id {id} not found for scaling"))?;
        let props = {
            let handle = node.to_handle(&self.tree);
            child_named(&handle, "Properties70").map(|p| p.node_id())
        }
        .with_context(|| format!("object id {id} has no Properties70"))?;

        let scaling = props
            .to_handle(&self.tree)
            .children()
            .find(|p| {
                p.name() == "P" && p.attributes().first().and_then(as_str) == Some("Lcl Scaling")
            })
            .map(|p| p.node_id())
            .with_context(|| format!("object id {id} has no Lcl Scaling property"))?;

        for i in 4..=6 {
            if let Some(attr) = self.tree.get_attribute_mut(scaling, i)
                && let Some(v) = as_f64(attr)
            {
                *attr = AttributeValue::F64(v * factor);
            }
        }
        Ok(())
    }

    /// Serialize the current tree to a binary FBX byte buffer.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Cursor::new(Vec::new()), self.version)
            .map_err(|e| anyhow::anyhow!("initializing FBX writer: {e}"))?;
        writer
            .write_tree(&self.tree)
            .map_err(|e| anyhow::anyhow!("writing FBX tree: {e}"))?;
        let sink = writer
            .finalize_and_flush(&FbxFooter::default())
            .map_err(|e| anyhow::anyhow!("finalizing FBX: {e}"))?;
        Ok(sink.into_inner())
    }

    /// Write the current tree to a binary FBX file.
    pub fn write(&self, path: &Path) -> Result<()> {
        let bytes = self.to_bytes()?;
        std::fs::write(path, bytes).with_context(|| format!("writing FBX file {}", path.display()))
    }
}

fn read_objects(root: &NodeHandle) -> Vec<FbxObject> {
    let mut out = Vec::new();
    let Some(objects) = child_named(root, "Objects") else {
        return out;
    };
    for node in objects.children() {
        let attrs = node.attributes();
        let id = match attrs.first().and_then(as_i64) {
            Some(id) => id,
            None => continue,
        };
        let (name, class) = attrs
            .get(1)
            .and_then(as_str)
            .map(split_name_class)
            .unwrap_or_default();
        let subclass = attrs
            .get(2)
            .and_then(as_str)
            .unwrap_or_default()
            .to_string();

        let transform = if node.name() == "Model" {
            read_local_transform(&node)
        } else {
            LocalTransform::default()
        };

        out.push(FbxObject {
            id,
            node_name: node.name().to_string(),
            name,
            class,
            subclass,
            transform,
        });
    }
    out
}

fn read_connections(root: &NodeHandle) -> Vec<Connection> {
    let mut out = Vec::new();
    let Some(connections) = child_named(root, "Connections") else {
        return out;
    };
    for node in connections.children() {
        if node.name() != "C" {
            continue;
        }
        let attrs = node.attributes();
        let kind = match attrs.first().and_then(as_str) {
            Some(k) => k.to_string(),
            None => continue,
        };
        let child = attrs.get(1).and_then(as_i64).unwrap_or(0);
        let parent = attrs.get(2).and_then(as_i64).unwrap_or(0);
        let property = attrs.get(3).and_then(as_str).map(|s| s.to_string());
        out.push(Connection {
            kind,
            child,
            parent,
            property,
        });
    }
    out
}

fn read_global_settings(root: &NodeHandle) -> GlobalSettings {
    let mut settings = GlobalSettings::default();
    let Some(gs) = child_named(root, "GlobalSettings") else {
        return settings;
    };
    let Some(props) = child_named(&gs, "Properties70") else {
        return settings;
    };
    for p in props.children().filter(|n| n.name() == "P") {
        let attrs = p.attributes();
        let Some(key) = attrs.first().and_then(as_str) else {
            continue;
        };
        match key {
            "UnitScaleFactor" => settings.unit_scale_factor = attrs.get(4).and_then(as_f64),
            // Axes are small ints (0/1/2); `try_from` makes an out-of-range value `None` rather
            // than silently wrapping a bogus i64 into a plausible-looking axis index.
            "UpAxis" => {
                settings.up_axis = attrs
                    .get(4)
                    .and_then(as_i64)
                    .and_then(|v| i32::try_from(v).ok())
            }
            "FrontAxis" => {
                settings.front_axis = attrs
                    .get(4)
                    .and_then(as_i64)
                    .and_then(|v| i32::try_from(v).ok())
            }
            _ => {}
        }
    }
    settings
}

fn read_local_transform(model: &NodeHandle) -> LocalTransform {
    let mut t = LocalTransform::default();
    let Some(props) = child_named(model, "Properties70") else {
        return t;
    };
    for p in props.children().filter(|n| n.name() == "P") {
        let attrs = p.attributes();
        let Some(key) = attrs.first().and_then(as_str) else {
            continue;
        };
        // A vector `P` lays out: [name, type, label, flags, x, y, z].
        let vec3 = || -> Option<[f64; 3]> {
            Some([
                attrs.get(4).and_then(as_f64)?,
                attrs.get(5).and_then(as_f64)?,
                attrs.get(6).and_then(as_f64)?,
            ])
        };
        match key {
            "Lcl Translation" => t.translation = vec3(),
            "Lcl Rotation" => t.rotation = vec3(),
            "Lcl Scaling" => t.scaling = vec3(),
            _ => {}
        }
    }
    t
}

fn child_named<'a>(node: &'a NodeHandle, name: &str) -> Option<NodeHandle<'a>> {
    node.children().find(|c| c.name() == name)
}

/// Split an FBX name attribute of the form `Name\0\1Class` into `(name, class)`.
fn split_name_class(s: &str) -> (String, String) {
    match s.split_once("\u{0}\u{1}") {
        Some((name, class)) => (name.to_string(), class.to_string()),
        None => (s.to_string(), String::new()),
    }
}

fn as_i64(v: &AttributeValue) -> Option<i64> {
    match v {
        AttributeValue::I16(n) => Some(*n as i64),
        AttributeValue::I32(n) => Some(*n as i64),
        AttributeValue::I64(n) => Some(*n),
        _ => None,
    }
}

fn as_f64(v: &AttributeValue) -> Option<f64> {
    match v {
        AttributeValue::F32(n) => Some(*n as f64),
        AttributeValue::F64(n) => Some(*n),
        AttributeValue::I16(n) => Some(*n as f64),
        AttributeValue::I32(n) => Some(*n as f64),
        AttributeValue::I64(n) => Some(*n as f64),
        _ => None,
    }
}

fn as_str(v: &AttributeValue) -> Option<&str> {
    match v {
        AttributeValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fbxcel::tree_v7400;

    /// A tiny but realistic FBX tree: GlobalSettings, three skeleton `Model`s wired Hips ->
    /// LeftArm -> LeftForeArm via `OO` connections, and a baked uniform scale on the root.
    fn synthetic_tree() -> Tree {
        tree_v7400! {
            GlobalSettings: {
                Properties70: {
                    P: ["UnitScaleFactor", "double", "Number", "", 1.0f64] {},
                    P: ["UpAxis", "int", "Integer", "", 2i32] {},
                },
            },
            Objects: {
                Model: [100i64, "Hips\u{0}\u{1}Model", "LimbNode"] {
                    Properties70: {
                        P: ["Lcl Scaling", "Lcl Scaling", "", "A", 2.0f64, 2.0f64, 2.0f64] {},
                    },
                },
                Model: [101i64, "mixamorig:LeftArm\u{0}\u{1}Model", "LimbNode"] {},
                Model: [102i64, "mixamorig:LeftForeArm\u{0}\u{1}Model", "LimbNode"] {},
            },
            Connections: {
                C: ["OO", 101i64, 100i64] {},
                C: ["OO", 102i64, 101i64] {},
            },
        }
    }

    fn to_fbx_bytes(tree: &Tree) -> Vec<u8> {
        let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).unwrap();
        w.write_tree(tree).unwrap();
        w.finalize_and_flush(&FbxFooter::default())
            .unwrap()
            .into_inner()
    }

    fn doc() -> FbxDocument {
        FbxDocument::from_bytes(&to_fbx_bytes(&synthetic_tree())).unwrap()
    }

    /// Reparse after a no-op write: objects, connections, and settings survive the writer.
    #[test]
    fn round_trips_objects_and_connections() {
        let d = doc();
        let s = d.scene();
        assert_eq!(s.models().count(), 3);
        assert_eq!(s.object(101).unwrap().name, "mixamorig:LeftArm");
        assert_eq!(s.parent_of(101), Some(100));
        assert_eq!(s.parent_of(102), Some(101));
        assert_eq!(s.global_settings.unit_scale_factor, Some(1.0));
        assert_eq!(s.global_settings.up_axis, Some(2));

        let s2 = FbxDocument::from_bytes(&d.to_bytes().unwrap())
            .unwrap()
            .scene();
        assert_eq!(s2.models().count(), 3);
        assert_eq!(s2.parent_of(102), Some(101));
    }

    #[test]
    fn rename_preserves_class_and_id_refs() {
        let mut d = doc();
        d.rename_object(101, "LeftUpperArm").unwrap();
        let s = FbxDocument::from_bytes(&d.to_bytes().unwrap())
            .unwrap()
            .scene();
        let o = s.object(101).unwrap();
        assert_eq!(o.name, "LeftUpperArm");
        assert_eq!(o.class, "Model", "class suffix must be preserved");
        // Connections reference by id, so the hierarchy is untouched by the rename.
        assert_eq!(s.parent_of(101), Some(100));
        assert_eq!(s.parent_of(102), Some(101));
    }

    #[test]
    fn reparent_repoints_oo_connection() {
        let mut d = doc();
        // Move LeftForeArm (102) off LeftArm (101) and onto Hips (100).
        d.reparent_object(102, 100).unwrap();
        let s = FbxDocument::from_bytes(&d.to_bytes().unwrap())
            .unwrap()
            .scene();
        assert_eq!(s.parent_of(102), Some(100));
        assert_eq!(s.parent_of(101), Some(100));
    }

    /// A bare `OO` reparent edits only the connection — it does **not** adjust the child's local
    /// transform. The child keeps a transform authored against its old parent, so its *world* rest
    /// pose shifts by (new-parent-world − old-parent-world). This is exactly why `avatar-armature`
    /// treats reparenting as report-only: a correct reparent must recompose the local transform
    /// against the new parent (geometry work, including the PreRotation/pivots Mixamo/Maya emit),
    /// not relabel a connection.
    #[test]
    fn reparent_leaves_local_transform_untouched() {
        let tree = tree_v7400! {
            Objects: {
                Model: [1i64, "Hips\u{0}\u{1}Model", "LimbNode"] {},
                Model: [2i64, "Arm\u{0}\u{1}Model", "LimbNode"] {},
                Model: [3i64, "Hand\u{0}\u{1}Model", "LimbNode"] {
                    Properties70: {
                        P: ["Lcl Translation", "Lcl Translation", "", "A", 0.0f64, 1.4f64, 0.0f64] {},
                    },
                },
            },
            Connections: {
                C: ["OO", 2i64, 1i64] {},
                C: ["OO", 3i64, 1i64] {}, // Hand mis-parented onto Hips.
            },
        };
        let mut d = FbxDocument::from_bytes(&to_fbx_bytes(&tree)).unwrap();
        let before = d.scene().object(3).unwrap().transform.translation;

        d.reparent_object(3, 2).unwrap();

        let after = d.scene();
        assert_eq!(
            after.parent_of(3),
            Some(2),
            "the parent connection is repointed"
        );
        assert_eq!(
            after.object(3).unwrap().transform.translation,
            before,
            "the local transform is unchanged — so the bone's world rest pose would move"
        );
    }

    #[test]
    fn edits_global_settings_and_scale() {
        let mut d = doc();
        d.set_global_setting_f64("UnitScaleFactor", 100.0).unwrap();
        d.set_global_setting_i32("UpAxis", 1).unwrap();
        d.scale_object(100, 0.5).unwrap();
        let s = FbxDocument::from_bytes(&d.to_bytes().unwrap())
            .unwrap()
            .scene();
        assert_eq!(s.global_settings.unit_scale_factor, Some(100.0));
        assert_eq!(s.global_settings.up_axis, Some(1));
        assert_eq!(
            s.object(100).unwrap().transform.scaling,
            Some([1.0, 1.0, 1.0])
        );
    }

    #[test]
    fn rename_unknown_id_errors() {
        let mut d = doc();
        assert!(d.rename_object(999, "Nope").is_err());
    }

    /// A minimal skinned mesh: one quad (4 control points) skinned to two bones, each cluster
    /// driving two control points. Bone0's `TransformLink` carries a known translation so the
    /// matrix round-trip can be asserted.
    fn skinned_quad_tree() -> Tree {
        // Row-major (FBX) translate-only matrix.
        let translate = |x: f64, y: f64, z: f64| {
            vec![
                1.0f64, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0, //
                x, y, z, 1.0,
            ]
        };
        let identity = translate(0.0, 0.0, 0.0);
        tree_v7400! {
            Objects: {
                Model: [10i64, "Mesh\u{0}\u{1}Model", "Mesh"] {},
                Geometry: [20i64, "Mesh\u{0}\u{1}Geometry", "Mesh"] {
                    Vertices: [vec![0.0f64, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]] {},
                    PolygonVertexIndex: [vec![0i32, 1, 2, -4]] {},
                },
                Model: [30i64, "Bone0\u{0}\u{1}Model", "LimbNode"] {},
                Model: [31i64, "Bone1\u{0}\u{1}Model", "LimbNode"] {},
                Deformer: [40i64, "Skin\u{0}\u{1}Deformer", "Skin"] {},
                SubDeformer: [50i64, "Cluster0\u{0}\u{1}SubDeformer", "Cluster"] {
                    Indexes: [vec![0i32, 1]] {},
                    Weights: [vec![1.0f64, 1.0]] {},
                    Transform: [identity.clone()] {},
                    TransformLink: [translate(5.0, 6.0, 7.0)] {},
                },
                SubDeformer: [51i64, "Cluster1\u{0}\u{1}SubDeformer", "Cluster"] {
                    Indexes: [vec![2i32, 3]] {},
                    Weights: [vec![1.0f64, 1.0]] {},
                    Transform: [identity] {},
                    TransformLink: [translate(0.0, 1.0, 0.0)] {},
                },
            },
            Connections: {
                C: ["OO", 20i64, 10i64] {}, // Geometry -> mesh Model
                C: ["OO", 40i64, 20i64] {}, // Skin -> Geometry
                C: ["OO", 50i64, 40i64] {}, // Cluster0 -> Skin
                C: ["OO", 51i64, 40i64] {}, // Cluster1 -> Skin
                C: ["OO", 30i64, 50i64] {}, // Bone0 -> Cluster0
                C: ["OO", 31i64, 51i64] {}, // Bone1 -> Cluster1
                C: ["OO", 31i64, 30i64] {}, // Bone1 child of Bone0
            },
        }
    }

    #[test]
    fn extracts_triangulated_geometry() {
        let d = FbxDocument::from_bytes(&to_fbx_bytes(&skinned_quad_tree())).unwrap();
        let meshes = d.meshes().unwrap();
        assert_eq!(meshes.len(), 1);
        let m = &meshes[0];
        assert_eq!(m.model_id, 10, "geometry resolves to its mesh Model");
        // A quad fan-triangulates into 2 triangles = 6 emitted vertices.
        assert_eq!(m.positions.len(), 6);
        assert_eq!(m.indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(m.control_point_of_vertex, vec![0, 1, 2, 0, 2, 3]);
        // Emitted vertex 1 is control point 1 = (1,0,0).
        assert_eq!(m.positions[1], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn extracts_skin_clusters_and_bind_matrices() {
        let d = FbxDocument::from_bytes(&to_fbx_bytes(&skinned_quad_tree())).unwrap();
        let m = &d.meshes().unwrap()[0];
        let skin = m.skin.as_ref().expect("mesh is skinned");
        assert_eq!(skin.clusters.len(), 2);

        let c0 = &skin.clusters[0];
        assert_eq!(c0.bone_id, 30, "cluster's OO-child Model is its bone");
        assert_eq!(c0.indexes, vec![0, 1]);
        assert_eq!(c0.weights, vec![1.0, 1.0]);
        // Row-major translate(5,6,7): translation occupies elements [12..=14].
        assert_eq!(&c0.transform_link[12..15], &[5.0, 6.0, 7.0]);

        assert_eq!(skin.clusters[1].bone_id, 31);
        assert_eq!(skin.clusters[1].indexes, vec![2, 3]);
    }

    #[test]
    fn skeleton_only_fbx_has_no_meshes() {
        // The existing skeleton-only fixture carries no Geometry nodes.
        let d = doc();
        assert!(d.meshes().unwrap().is_empty());
    }

    /// A two-triangle mesh with two materials (one per triangle, `ByPolygon`); the first material
    /// carries a diffuse colour and a diffuse texture (relative filename), the second only a colour.
    fn two_material_mesh_tree() -> Tree {
        tree_v7400! {
            Objects: {
                Model: [10i64, "Mesh\u{0}\u{1}Model", "Mesh"] {},
                Geometry: [20i64, "Mesh\u{0}\u{1}Geometry", "Mesh"] {
                    Vertices: [vec![0.0f64, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]] {},
                    // Two separate triangle polygons: (0,1,2) and (0,2,3).
                    PolygonVertexIndex: [vec![0i32, 1, -3, 0, 2, -4]] {},
                    LayerElementMaterial: [0i32] {
                        MappingInformationType: ["ByPolygon"] {},
                        ReferenceInformationType: ["IndexToDirect"] {},
                        Materials: [vec![0i32, 1]] {},
                    },
                },
                Material: [60i64, "Red\u{0}\u{1}Material", ""] {
                    Properties70: {
                        P: ["DiffuseColor", "Color", "", "A", 1.0f64, 0.0, 0.0] {},
                    },
                },
                Material: [61i64, "Green\u{0}\u{1}Material", ""] {
                    Properties70: {
                        P: ["DiffuseColor", "Color", "", "A", 0.0f64, 1.0, 0.0] {},
                    },
                },
                Texture: [70i64, "diffuse\u{0}\u{1}Texture", ""] {
                    RelativeFilename: ["textures/red.png"] {},
                },
            },
            Connections: {
                C: ["OO", 20i64, 10i64] {},          // Geometry -> Model
                C: ["OO", 60i64, 10i64] {},          // Material 0 -> Model (slot 0)
                C: ["OO", 61i64, 10i64] {},          // Material 1 -> Model (slot 1)
                C: ["OP", 70i64, 60i64, "DiffuseColor"] {}, // Texture -> Material 0 (diffuse)
            },
        }
    }

    #[test]
    fn extracts_materials_textures_and_per_triangle_slots() {
        let d = FbxDocument::from_bytes(&to_fbx_bytes(&two_material_mesh_tree())).unwrap();
        let m = &d.meshes().unwrap()[0];

        assert_eq!(m.materials.len(), 2, "two materials in OO-connection order");
        assert_eq!(m.materials[0].name, "Red");
        assert_eq!(m.materials[0].diffuse_color, Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(m.materials[1].diffuse_color, Some([0.0, 1.0, 0.0, 1.0]));

        let tex = m.materials[0]
            .texture
            .as_ref()
            .expect("slot 0 has a texture");
        assert_eq!(tex.relative.as_deref(), Some("textures/red.png"));
        assert!(m.materials[1].texture.is_none(), "slot 1 has no texture");

        // ByPolygon: triangle 0 → material 0, triangle 1 → material 1.
        assert_eq!(m.material_of_triangle, vec![0, 1]);
        assert_eq!(m.triangle_material(0), 0);
        assert_eq!(m.triangle_material(1), 1);
    }
}
