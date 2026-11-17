//! Time Beamer folder and PDF export of benchmark deck B (20 slides × 100
//! objects, 400 equations, 10 images).
//!
//! `cargo run --release --example export_timing -- <output-dir>`

use beamerx::assets::AssetStore;
use beamerx::latex::Toolchain;
use beamerx::persistence::SaveSnapshot;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "export-timing".into()));
    std::fs::create_dir_all(&out)?;
    let mut store = AssetStore::new_session()?;
    let doc = beamerx::bench::deck_b(&mut store);
    let assets = doc
        .referenced_assets()
        .into_iter()
        .filter_map(|id| store.get(&id).map(|m| (m.clone(), store.path_of(m))))
        .collect();
    let snap = SaveSnapshot { doc, assets };
    let t = Instant::now();
    beamerx::export::export_tex_package(&out.join("deck-b-beamer"), &snap)?;
    let tex_ms = t.elapsed().as_secs_f64() * 1000.0;
    let tc = Toolchain::detect().ok_or_else(|| anyhow::anyhow!("no LaTeX engine"))?;
    let t = Instant::now();
    beamerx::export::export_pdf(&out.join("deck-b.pdf"), &snap, &tc, &AtomicBool::new(false))?;
    let pdf_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{{\"objects\":{},\"beamer_folder_ms\":{tex_ms:.1},\"pdf_ms\":{pdf_ms:.0},\"engine\":\"{}\"}}",
        snap.doc.object_count(),
        tc.describe()
    );
    Ok(())
}
