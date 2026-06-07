//! Per-platform texture-memory estimation for one texture, from its image file + its `.meta` import
//! settings.
//!
//! VRChat's Texture Memory metric is the GPU VRAM the avatar's textures occupy — and it differs by
//! platform, because the same source is recompressed to a different GPU format (DXT/BC on PC,
//! ASTC/ETC2 on Android). We can't query the imported format offline, so we estimate it from what
//! the files *do* reveal:
//!
//! 1. the source image's pixel **dimensions** and whether it has an **alpha** channel (from the
//!    image header — PNG / PSD / TGA / JPEG),
//! 2. the **import settings** from the `.meta` (`maxTextureSize`, mipmaps, compression, and any
//!    explicit `textureFormat`), reading the platform's override (`Standalone` / `Android`) and
//!    falling back to the default platform,
//! 3. a **bytes-per-pixel** from the explicit format when one is set ([`bpp_for_format`]), else a
//!    per-platform Automatic-compression default (PC → DXT1/DXT5/BC7; Android → ASTC by quality).
//!
//! `bytes = effective_w · effective_h · bpp · mip_factor`, where the effective size scales the
//! source down so neither side exceeds `maxTextureSize` (aspect preserved) and `mip_factor` is 4/3
//! when mipmaps are on. This is an **estimate** — when a texture is left on Automatic we apply
//! Unity's default format choice, which can differ from the actual build — so treat it as a close
//! ballpark, not an exact byte count.

use std::path::Path;

use avatar_unity_yaml::{Yaml, field_bool, field_i64, parse_meta};

use crate::Platform;

/// What the image header tells us: pixel dimensions and whether an alpha channel is present.
struct ImageInfo {
    width: u64,
    height: u64,
    has_alpha: bool,
}

/// Estimate the VRAM (bytes) one texture occupies on `platform`, from its image file and `.meta`
/// text. `None` if the image format isn't one we can read dimensions from (so the caller can flag
/// it).
pub(crate) fn estimate_bytes(
    image_path: &Path,
    meta_text: &str,
    platform: Platform,
) -> Option<u64> {
    let bytes = std::fs::read(image_path).ok()?;
    let info = read_image_info(image_path, &bytes)?;
    let settings = ImportSettings::from_meta(meta_text, platform);

    let (w, h) = scaled_dims(info.width, info.height, settings.max_texture_size);
    let bpp = bytes_per_pixel(&settings, info.has_alpha, platform);
    let mip = if settings.mipmaps { 4.0 / 3.0 } else { 1.0 };

    Some((w as f64 * h as f64 * bpp * mip).round() as u64)
}

/// The import settings that affect VRAM, resolved from the `.meta` for one platform.
struct ImportSettings {
    max_texture_size: u64,
    mipmaps: bool,
    /// `textureCompression`: 0 Uncompressed, 1 Compressed, 2 CompressedHQ, 3 CompressedLQ.
    compression: i64,
    /// An explicit `textureFormat` (`TextureImporterFormat`), or `None` when Automatic (`-1`).
    format: Option<i64>,
}

impl ImportSettings {
    fn from_meta(meta_text: &str, platform: Platform) -> Self {
        // Sensible defaults if the .meta is missing/unreadable: Unity's import defaults.
        let mut s = ImportSettings {
            max_texture_size: 2048,
            mipmaps: true,
            compression: 1,
            format: None,
        };
        let Some(root) = parse_meta(meta_text) else {
            return s;
        };
        let ti = &root["TextureImporter"];

        // Mipmaps moved under `mipmaps:` in newer serializations; accept the older top-level too.
        s.mipmaps = field_bool(&ti["mipmaps"], "enableMipMap")
            .or_else(|| field_bool(ti, "enableMipMap"))
            .unwrap_or(true);

        // The platform whose settings govern this build target, then the top-level fallback.
        let target = match platform {
            Platform::Pc => "Standalone",
            Platform::Android => "Android",
        };
        let platform_entry = pick_platform(&ti["platformSettings"], target);

        s.max_texture_size = platform_entry
            .and_then(|p| field_i64(p, "maxTextureSize"))
            .or_else(|| field_i64(ti, "maxTextureSize"))
            .filter(|&v| v > 0)
            .map(|v| v as u64)
            .unwrap_or(s.max_texture_size);
        s.compression = platform_entry
            .and_then(|p| field_i64(p, "textureCompression"))
            .unwrap_or(s.compression);
        // textureFormat -1 (Automatic) means "let Unity choose"; treat that as no explicit format.
        s.format = platform_entry
            .and_then(|p| field_i64(p, "textureFormat"))
            .filter(|&f| f >= 0);

        s
    }
}

/// Choose the platform-settings entry that governs `target`: that entry if it's marked `overridden`,
/// otherwise `DefaultTexturePlatform`, otherwise the first entry.
fn pick_platform<'a>(platform_settings: &'a Yaml, target: &str) -> Option<&'a Yaml> {
    let list = platform_settings.as_vec()?;
    let by_target = |t: &str| list.iter().find(|p| p["buildTarget"].as_str() == Some(t));
    let overridden = by_target(target).filter(|p| field_bool(p, "overridden").unwrap_or(false));
    overridden
        .or_else(|| by_target("DefaultTexturePlatform"))
        .or_else(|| list.first())
}

/// Bytes-per-pixel for a texture, from its explicit format if set, else the platform's
/// Automatic-compression default.
fn bytes_per_pixel(settings: &ImportSettings, has_alpha: bool, platform: Platform) -> f64 {
    if let Some(bpp) = settings.format.and_then(bpp_for_format) {
        return bpp;
    }
    match platform {
        // PC Automatic: DXT1 (opaque) / DXT5 or BC7 (alpha); uncompressed = RGBA32.
        Platform::Pc => match settings.compression {
            0 => 4.0,
            _ if has_alpha => 1.0,
            _ => 0.5,
        },
        // Android Automatic: ASTC, block size by compression quality (HQ 4x4 / normal 6x6 / LQ 8x8).
        Platform::Android => match settings.compression {
            0 => 4.0,
            2 => astc_bpp(4), // CompressedHQ
            3 => astc_bpp(8), // CompressedLQ
            _ => astc_bpp(6), // Compressed (normal) — Unity's Android default.
        },
    }
}

/// Bytes-per-pixel of an ASTC block: a 128-bit (16-byte) block covers `n×n` texels.
fn astc_bpp(block: u64) -> f64 {
    16.0 / (block * block) as f64
}

/// Bytes-per-pixel for an explicit `TextureImporterFormat` value, or `None` if we don't model it
/// (the caller then falls back to the Automatic default).
fn bpp_for_format(format: i64) -> Option<f64> {
    Some(match format {
        // Uncompressed.
        1 | 63 => 1.0, // Alpha8 / R8
        2 | 62 => 2.0, // ARGB16 / RG16
        3..=5 => 4.0,  // RGB24 / RGBA32 / ARGB32 (RGB is stored 32-bit)
        // Desktop block compression.
        10 => 0.5,      // DXT1 (BC1)
        12 => 1.0,      // DXT5 (BC3)
        22 | 24 => 1.0, // RGB(A) BC6H (HDR) ~ 1 bpp
        25 => 1.0,      // BC7
        26 => 0.5,      // BC4
        27 => 1.0,      // BC5
        // Mobile ETC / ETC2.
        34 | 45 | 46 => 0.5, // ETC_RGB4 / ETC2_RGB4 / ETC2_RGB4_punchthrough
        47 => 1.0,           // ETC2_RGBA8
        // ASTC — RGB (48–53) and RGBA/HDR (54–59) share a block size, hence the same bpp.
        48 | 54 => astc_bpp(4),
        49 | 55 => astc_bpp(5),
        50 | 56 => astc_bpp(6),
        51 | 57 => astc_bpp(8),
        52 | 58 => astc_bpp(10),
        53 | 59 => astc_bpp(12),
        _ => return None,
    })
}

/// Source dimensions scaled so neither side exceeds `max` (aspect preserved). Most VRChat textures
/// are square, so this usually reduces to `min(dim, max)`.
fn scaled_dims(w: u64, h: u64, max: u64) -> (u64, u64) {
    let longest = w.max(h);
    if longest <= max || longest == 0 {
        return (w, h);
    }
    let scale = max as f64 / longest as f64;
    (
        ((w as f64 * scale).round() as u64).max(1),
        ((h as f64 * scale).round() as u64).max(1),
    )
}

/// Read image dimensions + alpha from a file's bytes, dispatching by extension.
fn read_image_info(path: &Path, bytes: &[u8]) -> Option<ImageInfo> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => png_info(bytes),
        Some("psd") => psd_info(bytes),
        Some("tga") => tga_info(bytes),
        Some("jpg" | "jpeg") => jpeg_info(bytes),
        _ => None, // unsupported source format (e.g. exr, tif) — caller flags it.
    }
}

fn be_u32(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at + 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as u64)
}

/// PNG: 8-byte signature, then an IHDR chunk — width/height (BE u32) at offsets 16/20, colour type
/// at 25 (6 = RGBA, 4 = grey+alpha carry alpha).
fn png_info(b: &[u8]) -> Option<ImageInfo> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if b.get(..8)? != SIG || b.get(12..16)? != b"IHDR" {
        return None;
    }
    let colour_type = *b.get(25)?;
    Some(ImageInfo {
        width: be_u32(b, 16)?,
        height: be_u32(b, 20)?,
        has_alpha: matches!(colour_type, 4 | 6),
    })
}

/// PSD: "8BPS" signature; channels (BE u16) at 12, height (BE u32) at 14, width (BE u32) at 18.
fn psd_info(b: &[u8]) -> Option<ImageInfo> {
    if b.get(..4)? != b"8BPS" {
        return None;
    }
    let channels = u16::from_be_bytes([*b.get(12)?, *b.get(13)?]);
    Some(ImageInfo {
        height: be_u32(b, 14)?,
        width: be_u32(b, 18)?,
        has_alpha: channels >= 4,
    })
}

/// TGA (no magic): width/height are LE u16 at offsets 12/14, pixel depth at 16 (32 ⇒ alpha).
fn tga_info(b: &[u8]) -> Option<ImageInfo> {
    let le_u16 = |at: usize| -> Option<u64> {
        let s = b.get(at..at + 2)?;
        Some(u16::from_le_bytes([s[0], s[1]]) as u64)
    };
    let depth = *b.get(16)?;
    Some(ImageInfo {
        width: le_u16(12)?,
        height: le_u16(14)?,
        has_alpha: depth == 32,
    })
}

/// JPEG: scan segments for a Start-Of-Frame marker (FFC0–FFCF, excluding the non-SOF C4/C8/CC);
/// height/width are BE u16 right after the marker's length + precision byte. JPEG has no alpha.
fn jpeg_info(b: &[u8]) -> Option<ImageInfo> {
    if b.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    // Need bytes `i..i+9` for the SOF read below; the upper bound is rechecked every iteration so a
    // crafted segment length can't walk us off the end (the index math is all checked/`get`).
    while i + 9 <= b.len() {
        if b.get(i) != Some(&0xFF) {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            // [FF marker][len:2][precision:1][height:2][width:2] — `i+8 < b.len()` here.
            let height = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u64;
            let width = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u64;
            return Some(ImageInfo {
                width,
                height,
                has_alpha: false,
            });
        }
        // Skip this segment using its big-endian length. Both the length read and the index advance
        // use checked arithmetic so a bogus length can't overflow `usize` and wrap back in-bounds.
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if len < 2 {
            return None;
        }
        match i.checked_add(2).and_then(|n| n.checked_add(len)) {
            Some(next) => i = next,
            None => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal PNG (signature + IHDR) with the given size and colour type.
    fn png(width: u32, height: u32, colour_type: u8) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&[0, 0, 0, 13]); // IHDR length
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&height.to_be_bytes());
        v.push(8); // bit depth
        v.push(colour_type);
        v.extend_from_slice(&[0, 0, 0]); // compression/filter/interlace
        v
    }

    #[test]
    fn reads_png_dimensions_and_alpha() {
        let rgba = png_info(&png(1024, 512, 6)).unwrap();
        assert_eq!((rgba.width, rgba.height), (1024, 512));
        assert!(rgba.has_alpha, "colour type 6 is RGBA");

        let rgb = png_info(&png(256, 256, 2)).unwrap();
        assert!(!rgb.has_alpha, "colour type 2 is RGB, no alpha");
    }

    #[test]
    fn jpeg_with_bogus_segment_length_does_not_panic() {
        // SOI, then an APP0 marker (FFE0) with a wildly oversized length. The advance must not
        // overflow/panic; we just want `None` (no SOF found) rather than an out-of-bounds index.
        let bogus = vec![0xFF, 0xD8, 0xFF, 0xE0, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(
            jpeg_info(&bogus),
            None,
            "oversized length walks past the end"
        );

        // A length of 0 (< 2) is rejected rather than looping forever.
        let zero_len = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(jpeg_info(&zero_len), None);

        // Truncated right at a would-be SOF header: bounds recheck keeps us from reading past end.
        let truncated = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11];
        assert_eq!(jpeg_info(&truncated), None);

        // A non-JPEG / empty input is simply rejected.
        assert_eq!(jpeg_info(&[]), None);
        assert_eq!(jpeg_info(&[0xFF, 0xD8]), None);
    }

    impl PartialEq for ImageInfo {
        fn eq(&self, o: &Self) -> bool {
            self.width == o.width && self.height == o.height && self.has_alpha == o.has_alpha
        }
    }

    impl std::fmt::Debug for ImageInfo {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ImageInfo")
                .field("width", &self.width)
                .field("height", &self.height)
                .field("has_alpha", &self.has_alpha)
                .finish()
        }
    }

    #[test]
    fn scaling_caps_the_longest_side() {
        assert_eq!(scaled_dims(4096, 4096, 2048), (2048, 2048));
        assert_eq!(scaled_dims(512, 512, 2048), (512, 512), "no upscaling");
        assert_eq!(
            scaled_dims(2048, 1024, 1024),
            (1024, 512),
            "aspect preserved"
        );
    }

    #[test]
    fn format_bpp_table_covers_the_common_gpu_formats() {
        assert_eq!(bpp_for_format(10), Some(0.5), "DXT1");
        assert_eq!(bpp_for_format(12), Some(1.0), "DXT5");
        assert_eq!(bpp_for_format(25), Some(1.0), "BC7");
        assert_eq!(bpp_for_format(47), Some(1.0), "ETC2_RGBA8");
        assert_eq!(bpp_for_format(45), Some(0.5), "ETC2_RGB4");
        assert_eq!(bpp_for_format(50), Some(16.0 / 36.0), "ASTC 6x6");
        assert_eq!(
            bpp_for_format(56),
            Some(16.0 / 36.0),
            "ASTC RGBA 6x6 == RGB 6x6"
        );
        assert_eq!(bpp_for_format(999), None, "unknown format falls back");
    }

    #[test]
    fn automatic_defaults_differ_by_platform() {
        let compressed = ImportSettings {
            max_texture_size: 2048,
            mipmaps: false,
            compression: 1,
            format: None,
        };
        // PC Automatic + alpha → DXT5/BC7 (1 bpp); Android Automatic → ASTC 6x6 (~0.444 bpp).
        assert_eq!(bytes_per_pixel(&compressed, true, Platform::Pc), 1.0);
        assert_eq!(
            bytes_per_pixel(&compressed, true, Platform::Android),
            16.0 / 36.0
        );
    }

    fn write_temp_png(label: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("avatar-tex-{}-{label}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tex.png");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        path
    }

    #[test]
    fn estimates_pc_and_android_for_the_same_texture() {
        // 1024×1024 RGBA, compressed + mipmaps, default platform only (both inherit it).
        let path = write_temp_png("both", &png(1024, 1024, 6));
        let meta = "\
TextureImporter:
  mipmaps:
    enableMipMap: 1
  maxTextureSize: 2048
  platformSettings:
  - buildTarget: DefaultTexturePlatform
    maxTextureSize: 2048
    textureCompression: 1
    textureFormat: -1
";
        let pc = estimate_bytes(&path, meta, Platform::Pc).unwrap();
        let android = estimate_bytes(&path, meta, Platform::Android).unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());

        let px = 1024.0_f64 * 1024.0 * (4.0 / 3.0);
        assert_eq!(pc, (px * 1.0).round() as u64, "PC DXT5/BC7 = 1 bpp");
        assert_eq!(
            android,
            (px * (16.0 / 36.0)).round() as u64,
            "Android ASTC 6x6"
        );
        assert!(android < pc, "ASTC 6x6 is denser than DXT5 here");
    }

    #[test]
    fn android_override_with_explicit_astc_block_is_used() {
        let path = write_temp_png("astc8", &png(2048, 2048, 6));
        // Android overrides to ASTC 8x8 (format 51) at 1024; PC stays default.
        let meta = "\
TextureImporter:
  mipmaps:
    enableMipMap: 0
  maxTextureSize: 2048
  platformSettings:
  - buildTarget: DefaultTexturePlatform
    maxTextureSize: 2048
    textureCompression: 1
    textureFormat: -1
  - buildTarget: Android
    overridden: 1
    maxTextureSize: 1024
    textureFormat: 51
";
        let android = estimate_bytes(&path, meta, Platform::Android).unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());

        // Capped to 1024², ASTC 8x8 (16/64 = 0.25 bpp), no mipmaps.
        assert_eq!(android, (1024.0 * 1024.0 * (16.0 / 64.0)) as u64);
    }
}
