//! Content-addressed image asset store.
//!
//! Imported images are kept as their **original compressed bytes on disk**
//! (in a private per-session directory), named by SHA-256 of the content.
//! Nothing here holds decoded pixels; previews are decoded lazily and
//! downsampled by the renderer's texture cache.

use crate::document::AssetId;
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Largest image file accepted for import (bytes).
pub const MAX_IMPORT_BYTES: u64 = 128 * 1024 * 1024;
/// Largest decoded image accepted (pixels). 4K is ~8.3 MP; 100 MP covers
/// high-end camera output while preventing decompression-bomb allocations.
pub const MAX_PIXELS: u64 = 100_000_000;
pub const MAX_SIDE: u32 = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
        }
    }

    pub fn media_type(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
        }
    }

    /// Identify by magic bytes; file extensions are not trusted.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(ImageFormat::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(ImageFormat::Jpeg)
        } else {
            None
        }
    }

    pub fn to_image_format(self) -> image::ImageFormat {
        match self {
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetMeta {
    pub id: AssetId,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    #[serde(default)]
    pub original_name: String,
}

impl AssetMeta {
    pub fn file_name(&self) -> String {
        format!("{}.{}", self.id.0, self.format.ext())
    }
}

pub fn hash_bytes(bytes: &[u8]) -> AssetId {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest.iter() {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    AssetId(s)
}

/// Validate image bytes and read their header dimensions without decoding.
pub fn probe(bytes: &[u8]) -> Result<(ImageFormat, u32, u32)> {
    let Some(format) = ImageFormat::sniff(bytes) else {
        bail!("unsupported image format (only PNG and JPEG are supported)");
    };
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format.to_image_format());
    let (w, h) = reader.into_dimensions().context("unreadable image header")?;
    if w == 0 || h == 0 {
        bail!("image has zero size");
    }
    if w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        bail!(
            "image is too large ({w}×{h} px); the limit is {} megapixels and {MAX_SIDE} px per side",
            MAX_PIXELS / 1_000_000
        );
    }
    Ok((format, w, h))
}

/// Read a file with an explicit size limit.
pub fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let f = fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let len = f.metadata()?.len();
    if len > limit {
        bail!(
            "{} is too large ({} MiB; limit {} MiB)",
            path.display(),
            len >> 20,
            limit >> 20
        );
    }
    let mut buf = Vec::with_capacity(len as usize);
    f.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        bail!("{} grew beyond the size limit while reading", path.display());
    }
    Ok(buf)
}

/// Write `bytes` to `path` atomically (temp file + fsync + rename).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Session-scoped asset store. Thread-safe operations that only need the
/// directory are free functions taking `dir`, so workers can run them
/// without access to the store itself.
pub struct AssetStore {
    dir: PathBuf,
    owns_dir: bool,
    entries: HashMap<AssetId, AssetMeta>,
}

impl AssetStore {
    /// Create a fresh private directory for this session's assets.
    pub fn new_session() -> Result<Self> {
        let base = std::env::temp_dir().join("BeamerX");
        fs::create_dir_all(&base)?;
        // Clock resolution alone is not unique enough (µs on macOS), so add
        // a process-wide counter.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = base.join(format!("session-{}-{nonce:x}-{n}", std::process::id()));
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            owns_dir: true,
            entries: HashMap::new(),
        })
    }

    /// Use an existing directory (not deleted on drop).
    pub fn with_dir(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            owns_dir: false,
            entries: HashMap::new(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn get(&self, id: &AssetId) -> Option<&AssetMeta> {
        self.entries.get(id)
    }

    pub fn path_of(&self, meta: &AssetMeta) -> PathBuf {
        self.dir.join(meta.file_name())
    }

    pub fn insert(&mut self, meta: AssetMeta) {
        self.entries.insert(meta.id.clone(), meta);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forget all entries (files stay; they are content-addressed and reused).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Validate, hash and store image bytes. Runs on worker threads.
    pub fn ingest_bytes(dir: &Path, bytes: &[u8], original_name: &str) -> Result<AssetMeta> {
        let (format, width, height) = probe(bytes)?;
        let id = hash_bytes(bytes);
        let meta = AssetMeta {
            id,
            format,
            width,
            height,
            bytes: bytes.len() as u64,
            original_name: original_name.chars().take(255).collect(),
        };
        let path = dir.join(meta.file_name());
        let already = fs::metadata(&path).map(|m| m.len() == meta.bytes).unwrap_or(false);
        if !already {
            write_atomic(&path, bytes)?;
        }
        Ok(meta)
    }

    /// Import an image file from disk. Runs on worker threads.
    pub fn import_file(dir: &Path, path: &Path) -> Result<AssetMeta> {
        let bytes = read_limited(path, MAX_IMPORT_BYTES)?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        Self::ingest_bytes(dir, &bytes, name).with_context(|| format!("cannot import {}", path.display()))
    }
}

impl Drop for AssetStore {
    fn drop(&mut self) {
        if self.owns_dir {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// Decode an asset for preview, downsampled so that neither side exceeds
/// `max_side` pixels. Returns straight-alpha RGBA8 pixels.
pub fn decode_preview(path: &Path, format: ImageFormat, max_side: u32) -> Result<(u32, u32, Vec<u8>)> {
    let bytes = read_limited(path, MAX_IMPORT_BYTES)?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format.to_image_format());
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_PIXELS * 4 + (64 << 20));
    reader.limits(limits);
    let img = reader.decode().context("cannot decode image")?;
    drop(bytes);
    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > max_side {
        let s = f64::from(max_side) / f64::from(w.max(h));
        let tw = ((f64::from(w) * s).round() as u32).max(1);
        let th = ((f64::from(h) * s).round() as u32).max(1);
        img.thumbnail(tw, th)
    } else {
        img
    };
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((w, h, rgba.into_raw()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn png_bytes(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x as u8).wrapping_add(seed), (y as u8).wrapping_mul(3), seed])
        });
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn ingest_dedupes_by_content() {
        let store = AssetStore::new_session().unwrap();
        let a = AssetStore::ingest_bytes(store.dir(), &png_bytes(20, 10, 1), "a.png").unwrap();
        let b = AssetStore::ingest_bytes(store.dir(), &png_bytes(20, 10, 1), "b.png").unwrap();
        let c = AssetStore::ingest_bytes(store.dir(), &png_bytes(20, 10, 2), "c.png").unwrap();
        assert_eq!(a.id, b.id);
        assert_ne!(a.id, c.id);
        assert_eq!((a.width, a.height, a.format), (20, 10, ImageFormat::Png));
        assert_eq!(fs::read_dir(store.dir()).unwrap().count(), 2);
    }

    #[test]
    fn rejects_non_images_and_bombs() {
        let store = AssetStore::new_session().unwrap();
        assert!(AssetStore::ingest_bytes(store.dir(), b"not an image", "x").is_err());
        // Valid PNG header claiming 30000x30000 px.
        let mut png = png_bytes(1, 1, 0);
        png[16..20].copy_from_slice(&30000u32.to_be_bytes());
        png[20..24].copy_from_slice(&30000u32.to_be_bytes());
        let e = AssetStore::ingest_bytes(store.dir(), &png, "bomb.png").unwrap_err();
        assert!(
            format!("{e:#}").contains("too large") || format!("{e:#}").contains("header"),
            "{e:#}"
        );
        // A truncated file must fail cleanly as well.
        let png = png_bytes(64, 64, 0);
        assert!(probe(&png[..40]).is_err() || decode_from(&png[..40]).is_err());
    }

    fn decode_from(bytes: &[u8]) -> Result<()> {
        let dir = AssetStore::new_session()?;
        let p = dir.dir().join("t.png");
        fs::write(&p, bytes)?;
        decode_preview(&p, ImageFormat::Png, 64).map(|_| ())
    }

    #[test]
    fn preview_is_downsampled() {
        let store = AssetStore::new_session().unwrap();
        let m = AssetStore::ingest_bytes(store.dir(), &png_bytes(400, 100, 3), "w.png").unwrap();
        let (w, h, px) = decode_preview(&store.path_of(&m), m.format, 128).unwrap();
        assert_eq!((w, h), (128, 32));
        assert_eq!(px.len(), (w * h * 4) as usize);
    }

    #[test]
    fn session_dir_removed_on_drop() {
        let store = AssetStore::new_session().unwrap();
        let dir = store.dir().to_path_buf();
        assert!(dir.exists());
        drop(store);
        assert!(!dir.exists());
    }
}
