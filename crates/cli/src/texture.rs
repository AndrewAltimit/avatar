//! Texture decoding + interning for the render command, plus the mesh-splitting that lets each
//! material slot become its own (single-texture) [`RenderMesh`].
//!
//! Pixels are resolved *here*, at the preview boundary — the importers (`avatar-fbx`) hand back
//! unresolved [`avatar_mesh::TextureRef`]s (a relative/absolute path and/or embedded bytes), and a
//! world scene hands back a `.mat`'s `_MainTex` asset path. Both funnel into a single
//! [`TextureSet`], which decodes once per distinct source (via the `image` crate), dedups by key,
//! and owns the [`avatar_render::Texture`] pool the [`avatar_render::Scene`] draws from. A
//! [`RenderMesh::texture`] is an index into that pool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use avatar_mesh::{MeshMaterial, RawMesh, TextureRef};
use avatar_render::{RenderMesh, Texture};
use glam::Mat4;

/// Owns the decoded texture pool and dedups decodes by a string key.
#[derive(Default)]
pub struct TextureSet {
    textures: Vec<Texture>,
    /// key → resolved pool index (`None` = we tried and failed/empty, cached so we don't retry).
    cache: HashMap<String, Option<usize>>,
}

impl TextureSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume into the pool the [`avatar_render::Scene`] will own.
    pub fn into_textures(self) -> Vec<Texture> {
        self.textures
    }

    /// Intern a decode under `key`, decoding via `bytes` only on a cache miss.
    fn intern(&mut self, key: String, bytes: impl FnOnce() -> Option<Vec<u8>>) -> Option<usize> {
        if let Some(&idx) = self.cache.get(&key) {
            return idx;
        }
        let idx = bytes()
            .as_deref()
            .and_then(decode_rgba)
            .map(|(width, height, rgba)| {
                self.textures.push(Texture {
                    width,
                    height,
                    rgba,
                });
                self.textures.len() - 1
            });
        self.cache.insert(key, idx);
        idx
    }

    /// Resolve a texture from a file on disk (e.g. a Unity material's `_MainTex` asset).
    pub fn resolve_file(&mut self, path: &Path) -> Option<usize> {
        let key = format!("file:{}", path.display());
        self.intern(key, || std::fs::read(path).ok())
    }

    /// Resolve an FBX material's diffuse texture: embedded bytes first, then an absolute authoring
    /// path, then the relative path (and its basename) searched near `fbx_dir`.
    pub fn resolve_fbx_material(&mut self, fbx_dir: &Path, mat: &MeshMaterial) -> Option<usize> {
        let tref = mat.texture.as_ref()?;
        let key = texture_ref_key(fbx_dir, tref);
        self.intern(key, || load_texture_ref(fbx_dir, tref))
    }
}

/// A resolved style for one material slot: its texture (if any) and tint colour.
pub struct SlotStyle {
    pub texture: Option<usize>,
    pub color: [f32; 4],
}

/// Split a mesh into one [`RenderMesh`] per *used* material slot, each carrying that slot's texture
/// and tint (from `style`). Single-material / unsplit meshes emit one mesh with no compaction; split
/// meshes compact their vertices so unused vertices don't bloat the merged buffer. `transform` is
/// the world placement applied to every emitted mesh.
pub fn split_by_material(
    m: &RawMesh,
    transform: Mat4,
    mut style: impl FnMut(usize) -> SlotStyle,
) -> Vec<RenderMesh> {
    let n_slots = m.material_slot_count();
    let split = !m.material_of_triangle.is_empty() && n_slots > 1;

    if !split {
        let s = style(0);
        let uvs = m.uvs.clone().unwrap_or_default();
        return vec![RenderMesh {
            positions: m.positions.clone(),
            normals: Vec::new(),
            uvs,
            indices: m.indices.clone(),
            color: s.color,
            texture: s.texture,
            transform,
        }];
    }

    let n_tri = m.indices.len() / 3;
    let mut out = Vec::new();
    for slot in 0..n_slots {
        let s = style(slot);
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        'tri: for t in 0..n_tri {
            if m.triangle_material(t) != slot {
                continue;
            }
            // Validate all three corner indices up front so a degenerate triangle (an index past
            // the position/UV arrays — possible on a malformed/corrupt mesh) is skipped whole,
            // instead of panicking or emitting a half-written triangle.
            let mut corners = [0u32; 3];
            for (k, corner) in corners.iter_mut().enumerate() {
                let vi = m.indices[t * 3 + k];
                let vu = vi as usize;
                if vu >= m.positions.len() || m.uvs.as_ref().is_some_and(|u| vu >= u.len()) {
                    eprintln!(
                        "warning: skipping triangle {t}: vertex index {vi} out of range \
                         (positions={}, uvs={:?})",
                        m.positions.len(),
                        m.uvs.as_ref().map(|u| u.len())
                    );
                    continue 'tri;
                }
                *corner = vi;
            }
            for vi in corners {
                let nv = *remap.entry(vi).or_insert_with(|| {
                    positions.push(m.positions[vi as usize]);
                    if let Some(u) = &m.uvs {
                        uvs.push(u[vi as usize]);
                    }
                    (positions.len() - 1) as u32
                });
                indices.push(nv);
            }
        }
        if indices.is_empty() {
            continue;
        }
        out.push(RenderMesh {
            positions,
            normals: Vec::new(),
            uvs,
            indices,
            color: s.color,
            texture: s.texture,
            transform,
        });
    }
    out
}

/// Decode image bytes to `(width, height, RGBA8)`. `None` on an unsupported/corrupt image.
fn decode_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some((w, h, img.into_raw()))
}

/// A stable cache key for a [`TextureRef`] in the context of `fbx_dir`.
fn texture_ref_key(fbx_dir: &Path, t: &TextureRef) -> String {
    if let Some(bytes) = &t.embedded {
        // Embedded textures may have no usable name; key by a cheap content fingerprint.
        return format!(
            "emb:{}:{:016x}",
            bytes.len(),
            avatar_unity_yaml::fnv1a(bytes)
        );
    }
    format!(
        "fbxtex:{}|{}|{}",
        fbx_dir.display(),
        t.relative.as_deref().unwrap_or(""),
        t.absolute.as_deref().unwrap_or("")
    )
}

/// Load a [`TextureRef`]'s bytes: embedded blob, else an existing absolute path, else the relative
/// path resolved against `fbx_dir`, else a basename search near `fbx_dir`.
fn load_texture_ref(fbx_dir: &Path, t: &TextureRef) -> Option<Vec<u8>> {
    if let Some(bytes) = &t.embedded {
        return Some(bytes.clone());
    }
    if let Some(abs) = &t.absolute {
        let p = Path::new(abs);
        if p.is_file() {
            return std::fs::read(p).ok();
        }
    }
    // The relative path (FBX stores Windows-style separators); normalize and resolve against the dir.
    let rel = t
        .relative
        .as_deref()
        .or(t.absolute.as_deref())
        .map(|s| s.replace('\\', "/"))?;
    let joined = fbx_dir.join(&rel);
    if joined.is_file() {
        return std::fs::read(joined).ok();
    }
    // Fall back to finding the basename near the FBX (Unity often keeps textures beside the model
    // or in a sibling "Textures" folder).
    let base = Path::new(&rel).file_name()?;
    let found = find_by_basename(fbx_dir, base, 2)?;
    std::fs::read(found).ok()
}

/// Shallow search (`max_depth` levels) under `dir` for a file whose name equals `base`.
fn find_by_basename(dir: &Path, base: &std::ffi::OsStr, max_depth: usize) -> Option<PathBuf> {
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < max_depth {
                    stack.push((p, depth + 1));
                }
            } else if p.file_name() == Some(base) {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_mesh::MeshMaterial;

    /// A 2-triangle quad split across two material slots (one triangle each).
    fn two_material_quad() -> RawMesh {
        RawMesh {
            model_id: 1,
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            normals: None,
            uvs: Some(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
            indices: vec![0, 1, 2, 0, 2, 3],
            control_point_of_vertex: vec![0, 1, 2, 0, 2, 3],
            skin: None,
            materials: vec![MeshMaterial::default(), MeshMaterial::default()],
            material_of_triangle: vec![0, 1],
        }
    }

    #[test]
    fn split_compacts_each_slot_to_its_own_mesh() {
        let m = two_material_quad();
        let out = split_by_material(&m, Mat4::IDENTITY, |slot| SlotStyle {
            texture: Some(slot),
            color: [slot as f32, 0.0, 0.0, 1.0],
        });
        assert_eq!(out.len(), 2, "one render mesh per used slot");
        for (slot, rm) in out.iter().enumerate() {
            assert_eq!(rm.indices.len(), 3, "one triangle per slot");
            // Compaction: only the 3 referenced vertices are kept.
            assert_eq!(rm.positions.len(), 3);
            assert_eq!(rm.uvs.len(), 3);
            assert_eq!(rm.texture, Some(slot));
            assert!(
                rm.indices
                    .iter()
                    .all(|&i| (i as usize) < rm.positions.len())
            );
        }
    }

    #[test]
    fn single_material_mesh_is_not_split() {
        let mut m = two_material_quad();
        m.materials.truncate(1);
        m.material_of_triangle.clear();
        let out = split_by_material(&m, Mat4::IDENTITY, |_| SlotStyle {
            texture: None,
            color: [1.0, 1.0, 1.0, 1.0],
        });
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].indices, m.indices,
            "unsplit mesh keeps its index buffer"
        );
        assert_eq!(out[0].positions.len(), 4);
    }

    #[test]
    fn split_skips_triangles_with_out_of_range_indices() {
        // Two material slots, two triangles — but slot 1's triangle references vertex index 99,
        // which doesn't exist. That triangle must be skipped, not panic; slot 0 still renders.
        let mut m = two_material_quad();
        m.indices = vec![0, 1, 2, 99, 2, 3];
        let out = split_by_material(&m, Mat4::IDENTITY, |_| SlotStyle {
            texture: None,
            color: [1.0, 1.0, 1.0, 1.0],
        });
        assert_eq!(out.len(), 1, "only the valid-index slot produces a mesh");
        assert_eq!(out[0].indices.len(), 3, "the good triangle survives");
        assert!(
            out[0]
                .indices
                .iter()
                .all(|&i| (i as usize) < out[0].positions.len())
        );
    }

    #[test]
    fn split_skips_triangle_when_uv_index_out_of_range() {
        // Positions cover the index, but the UV array is short — the triangle must still be skipped
        // rather than panicking on the UV lookup.
        let mut m = two_material_quad();
        m.uvs = Some(vec![[0.0, 0.0], [1.0, 0.0]]); // only 2 UVs for 4 positions
        let out = split_by_material(&m, Mat4::IDENTITY, |_| SlotStyle {
            texture: None,
            color: [1.0, 1.0, 1.0, 1.0],
        });
        // Slot 0's triangle (0,1,2) needs UV index 2, which is missing → skipped.
        // Slot 1's triangle (0,2,3) also needs missing UVs → skipped. Result: no meshes.
        assert!(out.iter().all(|rm| rm.uvs.len() == rm.positions.len()));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode_rgba(b"not an image").is_none());
    }

    #[test]
    fn embedded_texture_decodes_and_dedups() {
        // A 1×1 red PNG, embedded twice → decoded once, same pool index.
        let png = make_1x1_png();
        let mut set = TextureSet::new();
        let mat = MeshMaterial {
            name: "m".into(),
            diffuse_color: None,
            texture: Some(TextureRef {
                relative: None,
                absolute: None,
                embedded: Some(png),
            }),
        };
        let dir = Path::new("/nonexistent");
        let a = set.resolve_fbx_material(dir, &mat);
        let b = set.resolve_fbx_material(dir, &mat);
        assert_eq!(a, Some(0));
        assert_eq!(b, Some(0), "identical embedded bytes dedup to one texture");
        assert_eq!(set.into_textures().len(), 1);
    }

    /// Encode a 1×1 red RGBA PNG using the `image` crate (so the decode path is exercised end-to-end).
    fn make_1x1_png() -> Vec<u8> {
        let img = image::RgbaImage::from_raw(1, 1, vec![255, 0, 0, 255]).unwrap();
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }
}
