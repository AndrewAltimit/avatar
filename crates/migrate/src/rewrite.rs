//! Prefab-level rewrite operations — strip a subtree, remove / retype / add a component, poke a
//! scalar — expressed over fileIDs and applied through [`EditableUnityFile`] so every byte the
//! migration does not touch is preserved (fileIDs, references, key order, formatting).
//!
//! The [`Scene`] snapshot taken at construction is what the ops navigate by (which Transforms
//! are under which, which components a GameObject lists); it is not updated after edits, which
//! is fine because fileIDs are stable and each op re-resolves document indices and list
//! positions from the live text right before splicing.

use anyhow::{Context, Result, bail};
use avatar_unity_yaml::{EditableUnityFile, Scalar, UnityFile, parse_path};

use crate::scene::Scene;

/// A prefab held for rewriting.
pub struct PrefabRewriter {
    file: EditableUnityFile,
    scene: Scene,
    /// Human-readable log of what was done (for the migration report).
    pub log: Vec<String>,
}

impl PrefabRewriter {
    /// Parse `text` (a `.prefab`).
    pub fn new(text: &str) -> Result<Self> {
        let file = EditableUnityFile::parse(text).context("parsing prefab for rewriting")?;
        let parsed = UnityFile::parse(text)?;
        let scene = Scene::from_file(&parsed)?;
        Ok(PrefabRewriter {
            file,
            scene,
            log: Vec::new(),
        })
    }

    /// The scene snapshot (as of construction).
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The current text.
    pub fn text(&self) -> &str {
        self.file.text()
    }

    /// Finish, returning the rewritten prefab text.
    pub fn into_string(self) -> String {
        self.file.into_string()
    }

    fn doc(&self, file_id: i64) -> Result<usize> {
        self.file
            .doc_by_file_id(file_id)
            .with_context(|| format!("no document with fileID {file_id} (already removed?)"))
    }

    /// Remove Transform `transform`'s whole subtree: every GameObject, Transform and component
    /// document under it, and its entry in the parent's `m_Children`. Returns the number of
    /// documents removed.
    pub fn strip_subtree(&mut self, transform: i64) -> Result<usize> {
        let tr = self
            .scene
            .transforms
            .get(&transform)
            .with_context(|| format!("no Transform {transform}"))?
            .clone();
        let name = self.scene.name_of_transform(transform).to_string();
        let mut removed = 0;
        for t in self.scene.descendants(transform) {
            let Some(node) = self.scene.transforms.get(&t) else {
                continue;
            };
            let mut ids = vec![t];
            if let Some(go) = self.scene.game_objects.get(&node.game_object) {
                ids.push(go.file_id);
                ids.extend(go.components.iter().copied().filter(|c| *c != t));
            }
            for id in ids {
                if let Some(idx) = self.file.doc_by_file_id(id) {
                    self.file.remove_document(idx)?;
                    removed += 1;
                }
            }
        }
        if tr.parent != 0 {
            self.remove_list_ref(tr.parent, "m_Children", transform)?;
        }
        self.log.push(format!(
            "stripped subtree '{}' ({removed} objects)",
            if name.is_empty() {
                transform.to_string()
            } else {
                name
            }
        ));
        Ok(removed)
    }

    /// Remove component `component`: its document and its `m_Component` entry on the owner.
    pub fn remove_component(&mut self, component: i64) -> Result<()> {
        let owner = self
            .scene
            .owner_of(component)
            .with_context(|| format!("component {component} has no m_GameObject"))?;
        let idx = self.doc(component)?;
        let kind = self.file.documents()[idx].type_name.clone();
        self.file.remove_document(idx)?;
        self.remove_list_ref_field(owner, "m_Component", "component", component)?;
        self.log.push(format!(
            "removed {kind} from '{}'",
            self.scene
                .path_of(self.scene.game_objects[&owner].transform)
        ));
        Ok(())
    }

    /// Replace component `component`'s document with a new class + body at the **same fileID**,
    /// so the owner's `m_Component` slot and any references to it stay valid.
    pub fn retype_component(
        &mut self,
        component: i64,
        class_id: u32,
        body: &str,
        what: &str,
    ) -> Result<()> {
        let idx = self.doc(component)?;
        let old_class = self.file.documents()[idx].class_id;
        if old_class != class_id {
            self.file.retag_document(idx, class_id, component)?;
        }
        let idx = self.doc(component)?;
        self.file.replace_document_body(idx, body)?;
        self.log.push(what.to_string());
        Ok(())
    }

    /// Append a new component document (class `class_id`, `body`) and register it on
    /// `game_object`'s `m_Component` list. The fileID is derived from `seed` (stable across
    /// runs) and bumped past any collision. Returns the new fileID.
    pub fn add_component(
        &mut self,
        game_object: i64,
        class_id: u32,
        body: &str,
        seed: &str,
    ) -> Result<i64> {
        let mut id = derive_file_id(seed);
        while self.file.doc_by_file_id(id).is_some() {
            id += 1;
        }
        // The body's m_GameObject must point at the owner; callers render it that way, but check.
        if !body.contains(&format!("m_GameObject: {{fileID: {game_object}}}")) {
            bail!("component body does not reference its owner GameObject {game_object}");
        }
        self.file.append_document(class_id, id, body)?;
        let go_idx = self.doc(game_object)?;
        self.file.append_sequence_item(
            go_idx,
            &parse_path("m_Component"),
            &format!("component: {{fileID: {id}}}"),
        )?;
        Ok(id)
    }

    /// Create a new, empty GameObject named `name` as the last child of Transform `parent`
    /// (identity local transform). Returns `(game_object_id, transform_id)`.
    pub fn add_child_game_object(&mut self, parent: i64, name: &str) -> Result<(i64, i64)> {
        let parent_tr = self
            .scene
            .transforms
            .get(&parent)
            .with_context(|| format!("no Transform {parent}"))?;
        let root_order = parent_tr.children.len();
        let mut go_id = derive_file_id(&format!("go/{name}/{parent}"));
        while self.file.doc_by_file_id(go_id).is_some() {
            go_id += 1;
        }
        let mut tr_id = derive_file_id(&format!("transform/{name}/{parent}"));
        while self.file.doc_by_file_id(tr_id).is_some() || tr_id == go_id {
            tr_id += 1;
        }
        let go_body = format!(
            "GameObject:\n  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {{fileID: 0}}\n  m_PrefabInstance: {{fileID: 0}}\n  m_PrefabAsset: {{fileID: 0}}\n  serializedVersion: 6\n  m_Component:\n  - component: {{fileID: {tr_id}}}\n  m_Layer: 0\n  m_Name: {name}\n  m_TagString: Untagged\n  m_Icon: {{fileID: 0}}\n  m_NavMeshLayer: 0\n  m_StaticEditorFlags: 0\n  m_IsActive: 1\n"
        );
        let tr_body = format!(
            "Transform:\n  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {{fileID: 0}}\n  m_PrefabInstance: {{fileID: 0}}\n  m_PrefabAsset: {{fileID: 0}}\n  m_GameObject: {{fileID: {go_id}}}\n  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}\n  m_LocalPosition: {{x: 0, y: 0, z: 0}}\n  m_LocalScale: {{x: 1, y: 1, z: 1}}\n  m_Children: []\n  m_Father: {{fileID: {parent}}}\n  m_RootOrder: {root_order}\n  m_LocalEulerAnglesHint: {{x: 0, y: 0, z: 0}}\n"
        );
        self.file.append_document(1, go_id, &go_body)?;
        self.file.append_document(4, tr_id, &tr_body)?;
        let pidx = self.doc(parent)?;
        self.file.append_sequence_item(
            pidx,
            &parse_path("m_Children"),
            &format!("{{fileID: {tr_id}}}"),
        )?;
        // Keep the snapshot usable for later ops on the new objects.
        self.scene.transforms.insert(
            tr_id,
            crate::scene::Transform {
                file_id: tr_id,
                game_object: go_id,
                parent,
                children: Vec::new(),
                local: crate::math::Trs::default(),
            },
        );
        self.scene.game_objects.insert(
            go_id,
            crate::scene::GameObject {
                file_id: go_id,
                name: name.to_string(),
                transform: tr_id,
                components: vec![tr_id],
                active: true,
            },
        );
        if let Some(p) = self.scene.transforms.get_mut(&parent) {
            p.children.push(tr_id);
        }
        self.log.push(format!(
            "added empty GameObject '{name}' under '{}'",
            self.scene.path_of(parent)
        ));
        Ok((go_id, tr_id))
    }

    /// Move Transform `transform` under `new_parent` (appended as its last child), keeping its
    /// *local* transform — so the caller wants a new parent whose local pose is identity relative
    /// to the old one, or accepts the world-space change.
    pub fn reparent(&mut self, transform: i64, new_parent: i64) -> Result<()> {
        let old_parent = self
            .scene
            .transforms
            .get(&transform)
            .with_context(|| format!("no Transform {transform}"))?
            .parent;
        if old_parent != 0 {
            self.remove_list_ref(old_parent, "m_Children", transform)?;
        }
        let pidx = self.doc(new_parent)?;
        self.file.append_sequence_item(
            pidx,
            &parse_path("m_Children"),
            &format!("{{fileID: {transform}}}"),
        )?;
        let tidx = self.doc(transform)?;
        self.file
            .set_reference(tidx, &parse_path("m_Father"), new_parent, None, 0)?;
        if let Some(op) = self.scene.transforms.get_mut(&old_parent) {
            op.children.retain(|c| *c != transform);
        }
        if let Some(np) = self.scene.transforms.get_mut(&new_parent) {
            np.children.push(transform);
        }
        if let Some(t) = self.scene.transforms.get_mut(&transform) {
            t.parent = new_parent;
        }
        Ok(())
    }

    /// Set a scalar field on document `file_id` (e.g. `m_ApplyRootMotion` = 0).
    pub fn set_scalar(&mut self, file_id: i64, path: &str, value: Scalar) -> Result<()> {
        let idx = self.doc(file_id)?;
        self.file.set_scalar(idx, &parse_path(path), value)
    }

    /// Set a reference field on document `file_id`.
    pub fn set_reference(
        &mut self,
        file_id: i64,
        path: &str,
        target: i64,
        guid: Option<&str>,
        asset_type: i64,
    ) -> Result<()> {
        let idx = self.doc(file_id)?;
        self.file
            .set_reference(idx, &parse_path(path), target, guid, asset_type)
    }

    /// Remove the `- {fileID: target}` element from `list` on document `doc_id`.
    fn remove_list_ref(&mut self, doc_id: i64, list: &str, target: i64) -> Result<()> {
        let idx = self.doc(doc_id)?;
        let path = parse_path(list);
        let items = self.file.sequence_items(idx, &path)?;
        let needle = format!("{{fileID: {target}}}");
        let Some(pos) = items.iter().position(|it| it.contains(&needle)) else {
            bail!("{list} on {doc_id} has no entry for {target}");
        };
        self.file.remove_sequence_item(idx, &path, pos)
    }

    /// Remove the `- key: {fileID: target}` element from `list` on document `doc_id`.
    fn remove_list_ref_field(
        &mut self,
        doc_id: i64,
        list: &str,
        key: &str,
        target: i64,
    ) -> Result<()> {
        let idx = self.doc(doc_id)?;
        let path = parse_path(list);
        let items = self.file.sequence_items(idx, &path)?;
        let needle = format!("{key}: {{fileID: {target}}}");
        let Some(pos) = items.iter().position(|it| it.contains(&needle)) else {
            bail!("{list} on {doc_id} has no '{key}' entry for {target}");
        };
        self.file.remove_sequence_item(idx, &path, pos)
    }
}

/// A stable, positive prefab fileID from a seed string (FNV-1a masked into Unity's usual
/// 19-digit range and kept clear of the small ids Unity reserves for main objects).
pub fn derive_file_id(seed: &str) -> i64 {
    let h = avatar_unity_yaml::fnv1a(seed.as_bytes()) & 0x7fff_ffff_ffff_ffff;
    (h % 9_000_000_000_000_000_000u64 + 1_000_000_000_000_000_000u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFAB: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &100
GameObject:
  m_Component:
  - component: {fileID: 400}
  - component: {fileID: 9500}
  - component: {fileID: 18300}
  m_Name: Root
--- !u!4 &400
Transform:
  m_GameObject: {fileID: 100}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 0, y: 0, z: 0}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children:
  - {fileID: 401}
  - {fileID: 402}
  m_Father: {fileID: 0}
--- !u!95 &9500
Animator:
  m_GameObject: {fileID: 100}
  m_ApplyRootMotion: 1
--- !u!183 &18300
Cloth:
  m_GameObject: {fileID: 100}
  m_Enabled: 1
--- !u!1 &101
GameObject:
  m_Component:
  - component: {fileID: 401}
  - component: {fileID: 11401}
  m_Name: Vest
--- !u!4 &401
Transform:
  m_GameObject: {fileID: 101}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 0, y: 0, z: 0}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children:
  - {fileID: 403}
  m_Father: {fileID: 400}
--- !u!114 &11401
MonoBehaviour:
  m_GameObject: {fileID: 101}
  m_Script: {fileID: 11500000, guid: aaaa0000aaaa0000aaaa0000aaaa0000, type: 3}
  m_Name:
--- !u!1 &103
GameObject:
  m_Component:
  - component: {fileID: 403}
  m_Name: VestChild
--- !u!4 &403
Transform:
  m_GameObject: {fileID: 103}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 0, y: 0, z: 0}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children: []
  m_Father: {fileID: 401}
--- !u!1 &102
GameObject:
  m_Component:
  - component: {fileID: 402}
  m_Name: Hips
--- !u!4 &402
Transform:
  m_GameObject: {fileID: 102}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 0, y: 0, z: 0}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children: []
  m_Father: {fileID: 400}
";

    #[test]
    fn strip_subtree_removes_all_docs_and_parent_link() {
        let mut rw = PrefabRewriter::new(PREFAB).unwrap();
        let n = rw.strip_subtree(401).unwrap();
        // Vest: GO 101 + Transform 401 + MB 11401; VestChild: GO 103 + Transform 403.
        assert_eq!(n, 5);
        let t = rw.text();
        assert!(
            !t.contains("&401\n")
                && !t.contains("&11401")
                && !t.contains("&403")
                && !t.contains("m_Name: Vest")
        );
        assert!(t.contains("  m_Children:\n  - {fileID: 402}\n  m_Father: {fileID: 0}\n"));
        // Hips untouched.
        assert!(t.contains("m_Name: Hips"));
        UnityFile::parse(t).unwrap();
    }

    #[test]
    fn add_child_and_reparent_keep_lists_consistent() {
        let mut rw = PrefabRewriter::new(PREFAB).unwrap();
        // Group Hips (402) under a new child of the root (400).
        let (go, tr) = rw.add_child_game_object(400, "Group").unwrap();
        assert!(rw.text().contains(&format!("--- !u!1 &{go}\nGameObject:")));
        assert!(rw.text().contains("  m_Name: Group\n"));
        assert!(rw.text().contains(&format!(
            "  - {{fileID: 402}}\n  - {{fileID: {tr}}}\n  m_Father: {{fileID: 0}}\n"
        )));
        rw.reparent(402, tr).unwrap();
        // Root's children: 401 (Vest) then the group; Hips now under the group.
        assert!(rw.text().contains(&format!(
            "  m_Children:\n  - {{fileID: 401}}\n  - {{fileID: {tr}}}\n  m_Father: {{fileID: 0}}\n"
        )));
        assert!(
            rw.text()
                .contains("  m_Children:\n  - {fileID: 402}\n  m_Father: {fileID: 400}\n")
        );
        assert!(
            rw.text()
                .contains(&format!("  m_Children: []\n  m_Father: {{fileID: {tr}}}\n"))
        );
        assert_eq!(rw.scene().path_of(402), "Group/Hips");
        UnityFile::parse(rw.text()).unwrap();
    }

    #[test]
    fn remove_and_retype_and_add_components() {
        let mut rw = PrefabRewriter::new(PREFAB).unwrap();
        rw.remove_component(18300).unwrap();
        assert!(!rw.text().contains("Cloth:"));
        assert!(rw.text().contains("  m_Component:\n  - component: {fileID: 400}\n  - component: {fileID: 9500}\n  m_Name: Root\n"));

        rw.set_scalar(9500, "m_ApplyRootMotion", Scalar::Int(0))
            .unwrap();
        assert!(rw.text().contains("m_ApplyRootMotion: 0"));

        rw.retype_component(
            11401,
            114,
            "MonoBehaviour:\n  m_GameObject: {fileID: 101}\n  m_Script: {fileID: 1661641543, guid: 2a2c05204084d904aa4945ccff20d8e5, type: 3}\n  pull: 0.5\n",
            "DynamicBone -> PhysBone",
        )
        .unwrap();
        assert!(rw.text().contains("--- !u!114 &11401\nMonoBehaviour:\n  m_GameObject: {fileID: 101}\n  m_Script: {fileID: 1661641543"));

        let id = rw
            .add_component(
                102,
                114,
                "MonoBehaviour:\n  m_GameObject: {fileID: 102}\n  m_Name: New\n",
                "test/new",
            )
            .unwrap();
        assert!(rw.text().contains(&format!("--- !u!114 &{id}\n")));
        assert!(rw.text().contains(&format!("  m_Component:\n  - component: {{fileID: 402}}\n  - component: {{fileID: {id}}}\n  m_Name: Hips\n")));
        // Deterministic id.
        assert_eq!(id, derive_file_id("test/new"));
        UnityFile::parse(rw.text()).unwrap();
        assert!(
            rw.add_component(
                102,
                114,
                "MonoBehaviour:\n  m_GameObject: {fileID: 999}\n",
                "x"
            )
            .is_err()
        );
    }
}
