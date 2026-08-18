//! A read-only model of a prefab's object graph: GameObjects, their Transform tree, and every
//! component document — enough to *plan* a rewrite (which fileIDs to strip, retype, add) and to
//! compose local transforms into avatar space. Built from [`avatar_unity_yaml::UnityFile`]; the
//! rewrite itself is applied by [`crate::rewrite`] on the raw text so nothing here is ever
//! re-serialized.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use avatar_unity_yaml::{UnityDocument, UnityFile, Yaml, field_f64, field_i64, field_str};

use crate::math::{Quat, Trs, Vec3};

/// Unity class ids used here.
pub const GAME_OBJECT: u32 = 1;
pub const TRANSFORM: u32 = 4;
pub const CAMERA: u32 = 20;
pub const ANIMATOR: u32 = 95;
pub const MONO_BEHAVIOUR: u32 = 114;
pub const SKINNED_MESH_RENDERER: u32 = 137;
pub const CAPSULE_COLLIDER: u32 = 136;
pub const CLOTH: u32 = 183;

/// A GameObject: name, its Transform, and its component fileIDs in `m_Component` order.
#[derive(Debug, Clone)]
pub struct GameObject {
    pub file_id: i64,
    pub name: String,
    pub transform: i64,
    pub components: Vec<i64>,
    pub active: bool,
}

/// A Transform: owner GameObject, parent/children links, local TRS.
#[derive(Debug, Clone)]
pub struct Transform {
    pub file_id: i64,
    pub game_object: i64,
    pub parent: i64,
    pub children: Vec<i64>,
    pub local: Trs,
}

/// The parsed prefab graph.
#[derive(Debug, Clone)]
pub struct Scene {
    pub docs: HashMap<i64, UnityDocument>,
    pub game_objects: HashMap<i64, GameObject>,
    pub transforms: HashMap<i64, Transform>,
    /// Transforms whose `m_Father` is 0, in file order.
    pub roots: Vec<i64>,
}

impl Scene {
    /// Build the graph from a parsed prefab/scene file.
    pub fn from_file(file: &UnityFile) -> Result<Self> {
        let mut docs = HashMap::new();
        let mut game_objects = HashMap::new();
        let mut transforms = HashMap::new();
        let mut roots = Vec::new();
        for d in &file.documents {
            docs.insert(d.file_id, d.clone());
            match d.class_id {
                GAME_OBJECT => {
                    let comps: Vec<i64> = d.body["m_Component"]
                        .as_vec()
                        .map(|v| {
                            v.iter()
                                .filter_map(|c| field_i64(&c["component"], "fileID"))
                                .collect()
                        })
                        .unwrap_or_default();
                    game_objects.insert(
                        d.file_id,
                        GameObject {
                            file_id: d.file_id,
                            name: d.name().unwrap_or("").to_string(),
                            transform: 0, // filled below
                            components: comps,
                            active: field_i64(&d.body, "m_IsActive").unwrap_or(1) != 0,
                        },
                    );
                }
                TRANSFORM => {
                    let parent = field_i64(&d.body["m_Father"], "fileID").unwrap_or(0);
                    let children: Vec<i64> = d.body["m_Children"]
                        .as_vec()
                        .map(|v| v.iter().filter_map(|c| field_i64(c, "fileID")).collect())
                        .unwrap_or_default();
                    if parent == 0 {
                        roots.push(d.file_id);
                    }
                    transforms.insert(
                        d.file_id,
                        Transform {
                            file_id: d.file_id,
                            game_object: field_i64(&d.body["m_GameObject"], "fileID").unwrap_or(0),
                            parent,
                            children,
                            local: Trs {
                                position: vec3(&d.body["m_LocalPosition"]),
                                rotation: quat(&d.body["m_LocalRotation"]),
                                scale: vec3_or_one(&d.body["m_LocalScale"]),
                            },
                        },
                    );
                }
                _ => {}
            }
        }
        for t in transforms.values() {
            if let Some(go) = game_objects.get_mut(&t.game_object) {
                go.transform = t.file_id;
            }
        }
        if roots.is_empty() {
            bail!("prefab has no root Transform (no Transform with m_Father 0)");
        }
        Ok(Scene {
            docs,
            game_objects,
            transforms,
            roots,
        })
    }

    /// The single avatar root Transform. Errors if the file has several roots (a scene, not a
    /// prefab).
    pub fn root(&self) -> Result<&Transform> {
        if self.roots.len() != 1 {
            bail!(
                "expected exactly one root Transform, found {} — is this a prefab?",
                self.roots.len()
            );
        }
        Ok(&self.transforms[&self.roots[0]])
    }

    /// The document with this fileID.
    pub fn doc(&self, file_id: i64) -> Option<&UnityDocument> {
        self.docs.get(&file_id)
    }

    /// The name of the GameObject that owns Transform `t`.
    pub fn name_of_transform(&self, t: i64) -> &str {
        self.transforms
            .get(&t)
            .and_then(|tr| self.game_objects.get(&tr.game_object))
            .map(|g| g.name.as_str())
            .unwrap_or("")
    }

    /// The GameObject a component document belongs to (its `m_GameObject`).
    pub fn owner_of(&self, component: i64) -> Option<i64> {
        self.doc(component)
            .and_then(|d| field_i64(&d.body["m_GameObject"], "fileID"))
    }

    /// The Transform of the GameObject owning `component`.
    pub fn transform_of_component(&self, component: i64) -> Option<i64> {
        self.owner_of(component)
            .and_then(|go| self.game_objects.get(&go))
            .map(|g| g.transform)
    }

    /// Hierarchy path of Transform `t` relative to the root (`Armature/Hips/Spine`); the root
    /// itself is `""`.
    pub fn path_of(&self, t: i64) -> String {
        let mut parts = Vec::new();
        let mut cur = t;
        while let Some(tr) = self.transforms.get(&cur) {
            if tr.parent == 0 {
                break;
            }
            parts.push(self.name_of_transform(cur).to_string());
            cur = tr.parent;
        }
        parts.reverse();
        parts.join("/")
    }

    /// Transforms whose GameObject is named `name`, in no particular order.
    pub fn find_transforms_by_name(&self, name: &str) -> Vec<i64> {
        self.transforms
            .values()
            .filter(|t| self.name_of_transform(t.file_id) == name)
            .map(|t| t.file_id)
            .collect()
    }

    /// The Transform whose GameObject is uniquely named `name`.
    pub fn transform_by_name(&self, name: &str) -> Result<i64> {
        let hits = self.find_transforms_by_name(name);
        match hits.len() {
            1 => Ok(hits[0]),
            0 => bail!("no object named '{name}' in the prefab"),
            n => bail!("{n} objects named '{name}' in the prefab; refer to it by path"),
        }
    }

    /// The Transform at hierarchy `path` (`Armature/Hips`), or by bare unique name if the path
    /// has no `/`.
    pub fn transform_by_path(&self, path: &str) -> Result<i64> {
        if !path.contains('/') {
            return self.transform_by_name(path);
        }
        let root = self.root()?;
        let mut cur = root.file_id;
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            let next = self.transforms[&cur]
                .children
                .iter()
                .copied()
                .find(|c| self.name_of_transform(*c) == seg)
                .with_context(|| format!("no child '{seg}' under '{}'", self.path_of(cur)))?;
            cur = next;
        }
        Ok(cur)
    }

    /// Every Transform in the subtree rooted at `t` (inclusive), depth-first.
    pub fn descendants(&self, t: i64) -> Vec<i64> {
        let mut out = Vec::new();
        let mut stack = vec![t];
        while let Some(id) = stack.pop() {
            if out.contains(&id) {
                continue; // cycle guard
            }
            out.push(id);
            if let Some(tr) = self.transforms.get(&id) {
                for c in tr.children.iter().rev() {
                    stack.push(*c);
                }
            }
        }
        out
    }

    /// Transform `t` composed up to (and including) the root — avatar-local space.
    pub fn world(&self, t: i64) -> Trs {
        let mut chain = Vec::new();
        let mut cur = t;
        let mut guard = 0;
        while let Some(tr) = self.transforms.get(&cur) {
            chain.push(tr.local);
            if tr.parent == 0 {
                break;
            }
            cur = tr.parent;
            guard += 1;
            if guard > 4096 {
                break;
            }
        }
        let mut acc = Trs::default();
        for local in chain.iter().rev() {
            acc = acc.then(*local);
        }
        acc
    }

    /// Component fileIDs on Transform `t`'s GameObject whose document has class `class_id`.
    pub fn components_of_class(&self, t: i64, class_id: u32) -> Vec<i64> {
        self.transforms
            .get(&t)
            .and_then(|tr| self.game_objects.get(&tr.game_object))
            .map(|go| {
                go.components
                    .iter()
                    .copied()
                    .filter(|c| self.doc(*c).is_some_and(|d| d.class_id == class_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every MonoBehaviour document, with its script GUID.
    pub fn monobehaviours(&self) -> impl Iterator<Item = (&UnityDocument, Option<&str>)> {
        self.docs
            .values()
            .filter(|d| d.class_id == MONO_BEHAVIOUR)
            .map(|d| (d, d.script_guid()))
    }
}

pub fn vec3(node: &Yaml) -> Vec3 {
    Vec3::new(
        field_f64(node, "x").unwrap_or(0.0),
        field_f64(node, "y").unwrap_or(0.0),
        field_f64(node, "z").unwrap_or(0.0),
    )
}

fn vec3_or_one(node: &Yaml) -> Vec3 {
    if node.is_badvalue() || node.is_null() {
        Vec3::ONE
    } else {
        vec3(node)
    }
}

pub fn quat(node: &Yaml) -> Quat {
    if node.is_badvalue() || node.is_null() {
        return Quat::IDENTITY;
    }
    Quat::new(
        field_f64(node, "x").unwrap_or(0.0),
        field_f64(node, "y").unwrap_or(0.0),
        field_f64(node, "z").unwrap_or(0.0),
        field_f64(node, "w").unwrap_or(1.0),
    )
    .normalized()
}

/// `field_str` re-export for callers reading component bodies.
pub fn str_field<'a>(node: &'a Yaml, key: &str) -> Option<&'a str> {
    field_str(node, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const PREFAB: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &100
GameObject:
  m_Component:
  - component: {fileID: 400}
  - component: {fileID: 9500}
  m_Name: Root
  m_IsActive: 1
--- !u!4 &400
Transform:
  m_GameObject: {fileID: 100}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 0, y: 0, z: 0}
  m_LocalScale: {x: 0.5, y: 0.5, z: 0.5}
  m_Children:
  - {fileID: 401}
  m_Father: {fileID: 0}
--- !u!95 &9500
Animator:
  m_GameObject: {fileID: 100}
  m_ApplyRootMotion: 1
--- !u!1 &101
GameObject:
  m_Component:
  - component: {fileID: 401}
  m_Name: Armature
  m_IsActive: 1
--- !u!4 &401
Transform:
  m_GameObject: {fileID: 101}
  m_LocalRotation: {x: -0.7071068, y: 0, z: 0, w: 0.7071068}
  m_LocalPosition: {x: 0, y: 1, z: 0}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children:
  - {fileID: 402}
  m_Father: {fileID: 400}
--- !u!1 &102
GameObject:
  m_Component:
  - component: {fileID: 402}
  m_Name: Hips
  m_IsActive: 1
--- !u!4 &402
Transform:
  m_GameObject: {fileID: 102}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 0, y: 0, z: 2}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children: []
  m_Father: {fileID: 401}
";

    #[test]
    fn graph_paths_and_world_transforms() {
        let file = UnityFile::parse(PREFAB).unwrap();
        let s = Scene::from_file(&file).unwrap();
        assert_eq!(s.root().unwrap().file_id, 400);
        assert_eq!(s.path_of(402), "Armature/Hips");
        assert_eq!(s.transform_by_path("Armature/Hips").unwrap(), 402);
        assert_eq!(s.transform_by_name("Hips").unwrap(), 402);
        assert_eq!(s.descendants(400), vec![400, 401, 402]);
        // Hips local (0,0,2) under a -90deg X Armature at (0,1,0), all under a 0.5-scaled root:
        // Blender Z-up 2 -> Unity +Y 2; ×0.5 -> y = 0.5 + 1.0 = 1.5.
        let w = s.world(402);
        assert!((w.position.y - 1.5).abs() < 1e-5, "{:?}", w.position);
        assert!(w.position.x.abs() < 1e-6 && w.position.z.abs() < 1e-6);
        assert_eq!(s.components_of_class(400, ANIMATOR), vec![9500]);
        assert_eq!(s.transform_of_component(9500), Some(400));
    }
}
