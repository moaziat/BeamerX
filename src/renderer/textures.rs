//! Bounded LRU cache of GPU textures for image previews and equation
//! rasters.
//!
//! * Images are decoded on worker threads and **downsampled** to a
//!   power-of-two "level" just above the on-screen pixel size, so a 12 MP
//!   photo shown as a 300 px thumbnail occupies a 512 px texture.
//! * While a better level is loading, the best already-loaded level is shown.
//! * Total decoded bytes are capped by a budget; least-recently-used
//!   textures are released (dropping a `TextureHandle` frees the GPU
//!   texture). Entries used in the current or previous frame are never
//!   evicted, so the cache cannot thrash within a frame.

use crate::assets::{self, AssetMeta};
use crate::document::AssetId;
use crate::jobs::WorkerPool;
use crate::math::{MathKey, MathRender};
use crate::platform::pdf_raster;
use egui::{ColorImage, TextureHandle, TextureId, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

pub const DEFAULT_BUDGET: usize = 128 * 1024 * 1024;
const MIN_LEVEL: u32 = 64;
const MAX_LEVEL: u32 = 4096;
/// Bound on queued decode jobs (backpressure); the rest is re-requested on
/// a later frame.
const MAX_IN_FLIGHT: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TexKey {
    Image(AssetId, u32),
    Math(MathKey, u16),
}

struct TexEntry {
    handle: TextureHandle,
    bytes: usize,
    last_used: u64,
    size: [usize; 2],
}

type Loaded = (TexKey, Result<(TextureHandle, [usize; 2], usize), String>);

pub struct TextureCache {
    ctx: egui::Context,
    entries: HashMap<TexKey, TexEntry>,
    image_levels: HashMap<AssetId, Vec<u32>>,
    in_flight: HashSet<TexKey>,
    failed: HashMap<AssetId, String>,
    bytes: usize,
    budget: usize,
    frame: u64,
    pool: WorkerPool,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    pub stats: CacheStats,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub decodes: u64,
    pub evictions: u64,
    pub peak_bytes: usize,
}

/// Power-of-two level covering `px`, capped by the image's own size.
pub fn image_level(px: f32, natural_max_side: u32, max_texture_side: u32) -> u32 {
    let cap = natural_max_side.min(MAX_LEVEL).min(max_texture_side).max(1);
    let want = (px.max(1.0).ceil() as u32).next_power_of_two().max(MIN_LEVEL);
    want.min(cap)
}

/// Quantized raster scale for equations: steps of 2^(1/4) (~19 %).
pub fn math_bucket(px_per_bp: f32) -> u16 {
    let b = (px_per_bp.max(0.05).log2() * 4.0).ceil() + 64.0;
    b.clamp(0.0, 200.0) as u16
}

pub fn bucket_scale(bucket: u16) -> f32 {
    2f32.powf((f32::from(bucket) - 64.0) / 4.0)
}

impl TextureCache {
    pub fn new(ctx: egui::Context, budget: usize) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx,
            entries: HashMap::new(),
            image_levels: HashMap::new(),
            in_flight: HashSet::new(),
            failed: HashMap::new(),
            bytes: 0,
            budget,
            frame: 0,
            pool: WorkerPool::new("decode", 2),
            tx,
            rx,
            stats: CacheStats::default(),
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn pending(&self) -> usize {
        self.in_flight.len()
    }

    /// Call once per frame before painting. Returns true if new textures
    /// arrived (caller should repaint).
    pub fn begin_frame(&mut self) -> bool {
        self.frame += 1;
        let mut arrived = false;
        while let Ok((key, res)) = self.rx.try_recv() {
            self.in_flight.remove(&key);
            arrived = true;
            match res {
                Ok((handle, size, bytes)) => {
                    if let TexKey::Image(id, level) = &key {
                        let v = self.image_levels.entry(id.clone()).or_default();
                        if !v.contains(level) {
                            v.push(*level);
                        }
                    }
                    self.bytes += bytes;
                    self.entries.insert(
                        key,
                        TexEntry {
                            handle,
                            bytes,
                            last_used: self.frame,
                            size,
                        },
                    );
                }
                Err(msg) => {
                    if let TexKey::Image(id, _) = key {
                        self.failed.insert(id, msg);
                    }
                }
            }
        }
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.bytes);
        self.evict();
        arrived
    }

    fn evict(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut lru: Vec<(u64, TexKey)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.last_used + 1 < self.frame)
            .map(|(k, e)| (e.last_used, k.clone()))
            .collect();
        lru.sort_by_key(|(t, _)| *t);
        for (_, key) in lru {
            if self.bytes <= self.budget * 9 / 10 {
                break;
            }
            self.remove(&key);
            self.stats.evictions += 1;
        }
    }

    fn remove(&mut self, key: &TexKey) {
        if let Some(e) = self.entries.remove(key) {
            self.bytes -= e.bytes;
            if let TexKey::Image(id, level) = key
                && let Some(v) = self.image_levels.get_mut(id)
            {
                v.retain(|l| l != level);
                if v.is_empty() {
                    self.image_levels.remove(id);
                }
            }
        }
    }

    /// Release everything (e.g. when a different document is opened).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.image_levels.clear();
        self.failed.clear();
        self.bytes = 0;
    }

    pub fn image_error(&self, id: &AssetId) -> Option<&str> {
        self.failed.get(id).map(String::as_str)
    }

    /// Texture for an image displayed at `px` physical pixels (longest
    /// side). Returns the best available level and requests a better one.
    pub fn image(&mut self, meta: &AssetMeta, path: PathBuf, px: f32) -> Option<(TextureId, [usize; 2])> {
        if self.failed.contains_key(&meta.id) {
            return None;
        }
        let max_side = self.ctx.input(|i| i.max_texture_side) as u32;
        let level = image_level(px, meta.width.max(meta.height), max_side);
        let key = TexKey::Image(meta.id.clone(), level);
        if let Some(e) = self.entries.get_mut(&key) {
            e.last_used = self.frame;
            return Some((e.handle.id(), e.size));
        }
        self.request_image(key, meta, path);
        // Fallback: the closest loaded level (prefer larger).
        let levels = self.image_levels.get(&meta.id)?;
        let best = levels
            .iter()
            .copied()
            .min_by_key(|l| if *l >= level { *l - level } else { (level - *l) * 4 })?;
        let e = self.entries.get_mut(&TexKey::Image(meta.id.clone(), best))?;
        e.last_used = self.frame;
        Some((e.handle.id(), e.size))
    }

    fn request_image(&mut self, key: TexKey, meta: &AssetMeta, path: PathBuf) {
        if self.in_flight.contains(&key) || self.in_flight.len() >= MAX_IN_FLIGHT {
            return;
        }
        let TexKey::Image(_, level) = key else { return };
        self.in_flight.insert(key.clone());
        self.stats.decodes += 1;
        let (tx, ctx, format, name) = (
            self.tx.clone(),
            self.ctx.clone(),
            meta.format,
            meta.id.0[..12].to_string(),
        );
        self.pool.spawn(move || {
            let _span = tracing::debug_span!("decode_image", level).entered();
            let res = assets::decode_preview(&path, format, level)
                .map(|(w, h, px)| {
                    let img = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
                    drop(px);
                    let opts = TextureOptions {
                        mipmap_mode: Some(egui::TextureFilter::Linear),
                        ..TextureOptions::LINEAR
                    };
                    let bytes = (w as usize) * (h as usize) * 4 * 4 / 3;
                    (
                        ctx.load_texture(format!("img-{name}-{level}"), img, opts),
                        [w as usize, h as usize],
                        bytes,
                    )
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send((key, res));
            ctx.request_repaint();
        });
    }

    /// Texture for an equation rasterized at about `px_per_bp`.
    pub fn math(&mut self, render: &Arc<MathRender>, px_per_bp: f32) -> Option<TextureId> {
        let bucket = math_bucket(px_per_bp);
        let key = TexKey::Math(render.key, bucket);
        if let Some(e) = self.entries.get_mut(&key) {
            e.last_used = self.frame;
            return Some(e.handle.id());
        }
        if !self.in_flight.contains(&key) && self.in_flight.len() < MAX_IN_FLIGHT * 2 {
            self.in_flight.insert(key.clone());
            let (tx, ctx, r) = (self.tx.clone(), self.ctx.clone(), Arc::clone(render));
            self.pool.spawn(move || {
                let scale = bucket_scale(bucket);
                let res = pdf_raster::rasterize_alpha(&r.pdf, r.page, scale)
                    .map(|m| {
                        let size = [m.width as usize, m.height as usize];
                        // White glyphs with coverage alpha, tinted at paint time.
                        let img = ColorImage::from_rgba_premultiplied(
                            size,
                            &m.alpha.iter().flat_map(|&a| [a, a, a, a]).collect::<Vec<u8>>(),
                        );
                        let bytes = size[0] * size[1] * 4;
                        (
                            ctx.load_texture(format!("math-{}", r.key.hex()), img, TextureOptions::LINEAR),
                            size,
                            bytes,
                        )
                    })
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send((key, res));
                ctx.request_repaint();
            });
        }
        // Fallback: any loaded bucket for this equation.
        let fallback = self
            .entries
            .iter_mut()
            .filter(|(k, _)| matches!(k, TexKey::Math(mk, _) if *mk == render.key))
            .max_by_key(|(k, _)| match k {
                TexKey::Math(_, b) => *b,
                _ => 0,
            })?;
        fallback.1.last_used = self.frame;
        Some(fallback.1.handle.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_bounded_powers_of_two() {
        assert_eq!(image_level(300.0, 4000, 8192), 512);
        assert_eq!(image_level(10.0, 4000, 8192), 64);
        assert_eq!(image_level(3000.0, 4000, 8192), 4000);
        assert_eq!(image_level(9000.0, 12000, 8192), 4096);
        assert_eq!(image_level(9000.0, 12000, 2048), 2048);
        assert_eq!(image_level(500.0, 100, 8192), 100);
    }

    #[test]
    fn math_buckets_cover_requested_scale() {
        for s in [0.3f32, 1.0, 1.7, 2.0, 3.3, 8.0] {
            let b = bucket_scale(math_bucket(s));
            assert!(b >= s * 0.999 && b < s * 1.2, "{s} -> {b}");
        }
    }

    #[test]
    fn cache_respects_budget_and_evicts_lru() {
        let ctx = egui::Context::default();
        let mut cache = TextureCache::new(ctx.clone(), 1000);
        for i in 0..10u8 {
            let key = TexKey::Image(AssetId(format!("{i:064}")), 64);
            let img = ColorImage::filled([8, 8], egui::Color32::WHITE);
            let handle = ctx.load_texture("t", img, TextureOptions::LINEAR);
            cache.tx.send((key, Ok((handle, [8, 8], 256)))).unwrap();
            cache.begin_frame();
            cache.begin_frame();
            cache.begin_frame();
        }
        assert!(cache.bytes() <= 1000, "{}", cache.bytes());
        assert!(cache.stats.evictions >= 6);
    }
}
