//! `.beamerx` project packages.
//!
//! A project is a ZIP container:
//!
//! ```text
//! mimetype                  "application/x-beamerx" (stored, first entry)
//! project.json              versioned manifest + document (deflated)
//! assets/<sha256>.<ext>     original image bytes (stored, not recompressed)
//! ```
//!
//! Saving writes a temporary file next to the target, fsyncs it and renames
//! it over the target, so a crash or failure never destroys the previous
//! valid project. Loading treats the archive as untrusted: entry names are
//! never used as paths, sizes are bounded, and asset content is verified
//! against its hash.

use crate::assets::{AssetMeta, MAX_IMPORT_BYTES, hash_bytes};
use crate::document::Document;
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const FORMAT_NAME: &str = "beamerx";
pub const FORMAT_VERSION: u32 = 3;
pub const MIMETYPE: &str = "application/x-beamerx";
pub const EXTENSION: &str = "beamerx";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    format_version: u32,
    app_version: String,
    document: Document,
    assets: Vec<AssetMeta>,
}

/// Everything needed to write a project, detached from the live editor so
/// that saving can run on a worker thread.
#[derive(Clone)]
pub struct SaveSnapshot {
    pub doc: Document,
    /// Referenced assets and the on-disk files holding their bytes.
    pub assets: Vec<(AssetMeta, PathBuf)>,
}

pub struct Loaded {
    pub doc: Document,
    pub assets: Vec<AssetMeta>,
    /// Non-fatal problems (e.g. a corrupt image that was skipped).
    pub warnings: Vec<String>,
}

/// Write a project package atomically.
pub fn save_package(path: &Path, snap: &SaveSnapshot) -> Result<()> {
    let _span = tracing::info_span!("save_package", path = %path.display(), assets = snap.assets.len()).entered();
    let started = std::time::Instant::now();
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("project");
    let tmp = dir.join(format!(".{name}.saving-{}", std::process::id()));

    let result = (|| -> Result<()> {
        for (meta, src) in &snap.assets {
            let len = fs::metadata(src)
                .with_context(|| format!("image data for {} is missing", meta.original_name))?
                .len();
            if len != meta.bytes {
                bail!("image data for {} is corrupt (size mismatch)", meta.original_name);
            }
        }
        let file = fs::File::create(&tmp).with_context(|| format!("cannot write to {}", dir.display()))?;
        let mut zip = ZipWriter::new(std::io::BufWriter::new(file));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored)?;
        zip.write_all(MIMETYPE.as_bytes())?;

        let manifest = Manifest {
            format: FORMAT_NAME.into(),
            format_version: FORMAT_VERSION,
            app_version: env!("CARGO_PKG_VERSION").into(),
            document: snap.doc.clone(),
            assets: snap.assets.iter().map(|(m, _)| m.clone()).collect(),
        };
        zip.start_file("project.json", deflated)?;
        serde_json::to_writer(&mut zip, &manifest)?;

        for (meta, src) in &snap.assets {
            zip.start_file(
                format!("assets/{}", meta.file_name()),
                stored.large_file(meta.bytes >= u32::MAX as u64),
            )?;
            let mut f = fs::File::open(src)?;
            std::io::copy(&mut f, &mut zip)?;
        }
        let mut w = zip.finish()?;
        w.flush()?;
        let file = w.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    tracing::debug!(ms = started.elapsed().as_millis() as u64, ok = result.is_ok(), "saved");
    result
}

/// Bring an older manifest up to [`FORMAT_VERSION`].
///
/// Each future format change adds one `n => n+1` step here plus a frozen
/// fixture in `tests/fixtures` proving the old file still opens.
fn migrate(mut v: Value) -> Result<Value> {
    let format = v.get("format").and_then(Value::as_str).unwrap_or("");
    if format != FORMAT_NAME {
        bail!("not a BeamerX project");
    }
    let ver = v
        .get("format_version")
        .and_then(Value::as_u64)
        .context("missing format_version")?;
    if ver > u64::from(FORMAT_VERSION) {
        bail!(
            "this project was saved by a newer BeamerX (format {ver}); this version supports format {FORMAT_VERSION}"
        );
    }
    if ver < 1 {
        bail!("unsupported project format version {ver}");
    }
    let mut ver = ver;
    if ver == 1 {
        migrate_v1_to_v2(&mut v);
        ver = 2;
    }
    if ver == 2 {
        migrate_v2_to_v3(&mut v);
        ver = 3;
    }
    v["format_version"] = ver.into();
    Ok(v)
}

/// Format 2 removed the separate equation object: math is written inside
/// text boxes as `$…$`. Each v1 equation becomes a text box containing
/// `$\displaystyle …$` at the same position, size and colour.
fn migrate_v1_to_v2(v: &mut Value) {
    let Some(slides) = v.pointer_mut("/document/slides").and_then(Value::as_array_mut) else {
        return;
    };
    for slide in slides {
        let Some(objects) = slide.get_mut("objects").and_then(Value::as_array_mut) else {
            continue;
        };
        for o in objects {
            if o.get("type").and_then(Value::as_str) != Some("equation") {
                continue;
            }
            let source = o.get("source").and_then(Value::as_str).unwrap_or("").to_string();
            let font_size = o.get("font_size").cloned().unwrap_or_else(|| 20.0.into());
            let color = o.get("color").cloned().unwrap_or_else(|| "#000000".into());
            // Leave room so TeX does not wrap the formula.
            if let Some(w) = o.pointer("/rect/w").and_then(Value::as_f64) {
                o["rect"]["w"] = (w * 1.25 + 12.0).into();
            }
            let obj = o.as_object_mut().expect("checked object");
            obj.remove("source");
            obj.insert("type".into(), "text".into());
            obj.insert("text".into(), format!("$\\displaystyle {}$", source.trim()).into());
            obj.insert("font_size".into(), font_size);
            obj.insert("color".into(), color);
        }
    }
}

/// Format 3 replaced the box-wide `bullets` flag with per-paragraph
/// formats (lists, levels, styles) and added character spans.
fn migrate_v2_to_v3(v: &mut Value) {
    let Some(slides) = v.pointer_mut("/document/slides").and_then(Value::as_array_mut) else {
        return;
    };
    for slide in slides {
        let Some(objects) = slide.get_mut("objects").and_then(Value::as_array_mut) else {
            continue;
        };
        for o in objects {
            if o.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            let Some(obj) = o.as_object_mut() else { continue };
            let bullets = obj.remove("bullets").and_then(|b| b.as_bool()).unwrap_or(false);
            if bullets {
                let lines = obj
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.split('\n').count())
                    .unwrap_or(1);
                let para = serde_json::json!({ "list": "bullet" });
                obj.insert("paras".into(), Value::Array(vec![para; lines]));
            }
        }
    }
}

/// Parse and validate a manifest (`project.json`).
pub fn parse_manifest(bytes: &[u8]) -> Result<(Document, Vec<AssetMeta>)> {
    let v: Value = serde_json::from_slice(bytes).context("project.json is not valid JSON")?;
    let v = migrate(v)?;
    let m: Manifest = serde_json::from_value(v).context("project.json has an invalid structure")?;
    let mut doc = m.document;
    doc.validate_and_repair()
        .map_err(|e| anyhow::anyhow!("invalid document: {e}"))?;
    let mut assets = Vec::with_capacity(m.assets.len());
    let mut seen = std::collections::HashSet::new();
    for a in m.assets {
        if !a.id.is_valid() {
            bail!("invalid asset id in manifest");
        }
        if seen.insert(a.id.clone()) {
            assets.push(a);
        }
    }
    Ok((doc, assets))
}

/// Open a project package, extracting its assets into `asset_dir`.
pub fn load_package(path: &Path, asset_dir: &Path) -> Result<Loaded> {
    let _span = tracing::info_span!("load_package", path = %path.display()).entered();
    let started = std::time::Instant::now();
    let file = fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut zip = ZipArchive::new(std::io::BufReader::new(file)).context("not a BeamerX project (bad container)")?;

    let manifest_bytes = {
        let entry = zip
            .by_name("project.json")
            .context("not a BeamerX project (no project.json)")?;
        if entry.size() > MAX_MANIFEST_BYTES {
            bail!("project.json is unreasonably large");
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut buf)?;
        buf
    };
    let (doc, metas) = parse_manifest(&manifest_bytes)?;
    drop(manifest_bytes);

    let mut warnings = Vec::new();
    let mut assets = Vec::with_capacity(metas.len());
    for meta in metas {
        let dest = asset_dir.join(meta.file_name());
        if fs::metadata(&dest).map(|m| m.len() == meta.bytes).unwrap_or(false) {
            assets.push(meta);
            continue;
        }
        // The entry name is derived from the validated hash, never taken
        // from the archive, so it cannot traverse outside `asset_dir`.
        let res = (|| -> Result<()> {
            let entry = zip
                .by_name(&format!("assets/{}", meta.file_name()))
                .context("missing from package")?;
            if entry.size() > MAX_IMPORT_BYTES {
                bail!("too large");
            }
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.take(MAX_IMPORT_BYTES + 1).read_to_end(&mut buf)?;
            if hash_bytes(&buf) != meta.id {
                bail!("content does not match its checksum");
            }
            crate::assets::write_atomic(&dest, &buf)?;
            Ok(())
        })();
        match res {
            Ok(()) => assets.push(meta),
            Err(e) => warnings.push(format!("Image {} could not be loaded: {e:#}", display_name(&meta))),
        }
    }
    let known: std::collections::HashSet<_> = assets.iter().map(|a| a.id.clone()).collect();
    let missing = doc
        .referenced_assets()
        .into_iter()
        .filter(|a| !known.contains(a))
        .count();
    if missing > 0 {
        warnings.push(format!("{missing} image(s) are missing and will show as placeholders."));
    }
    tracing::debug!(
        ms = started.elapsed().as_millis() as u64,
        assets = assets.len(),
        "loaded"
    );
    Ok(Loaded { doc, assets, warnings })
}

fn display_name(m: &AssetMeta) -> String {
    if m.original_name.is_empty() {
        m.id.0[..12].to_string()
    } else {
        m.original_name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{AssetStore, tests::png_bytes};
    use crate::document::*;

    fn sample(store: &AssetStore) -> SaveSnapshot {
        let meta = AssetStore::ingest_bytes(store.dir(), &png_bytes(32, 16, 7), "plot.png").unwrap();
        let mut doc = Document::new();
        let sid = doc.slides[0].id;
        let ids: Vec<ObjectId> = (0..4).map(|_| doc.alloc_object_id()).collect();
        let s = doc.slide_mut(sid).unwrap();
        s.objects.push(Object {
            id: ids[0],
            rect: Rect::new(20.0, 20.0, 200.0, 30.0),
            kind: ObjectKind::Text(TextBox {
                text: "Hello & {world} 100%".into(),
                bold: true,
                ..Default::default()
            }),
        });
        s.objects.push(Object {
            id: ids[1],
            rect: Rect::new(20.0, 80.0, 120.0, 20.0),
            kind: ObjectKind::Text(TextBox {
                text: r"Area $\int_0^1 x^2\,dx$ and $$\sum_i x_i$$".into(),
                ..Default::default()
            }),
        });
        s.objects.push(Object {
            id: ids[2],
            rect: Rect::new(250.0, 20.0, 160.0, 80.0),
            kind: ObjectKind::Image(ImageRef { asset: meta.id.clone() }),
        });
        s.objects.push(Object {
            id: ids[3],
            rect: Rect::new(10.0, 200.0, 100.0, 0.0),
            kind: ObjectKind::Shape(Shape::new(ShapeKind::Arrow)),
        });
        let path = store.path_of(&meta);
        SaveSnapshot {
            doc,
            assets: vec![(meta, path)],
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let store = AssetStore::new_session().unwrap();
        let snap = sample(&store);
        let out = store.dir().join("deck.beamerx");
        save_package(&out, &snap).unwrap();
        let other = AssetStore::new_session().unwrap();
        let loaded = load_package(&out, other.dir()).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.doc, snap.doc);
        assert_eq!(loaded.assets, vec![snap.assets[0].0.clone()]);
        let restored = fs::read(other.dir().join(snap.assets[0].0.file_name())).unwrap();
        assert_eq!(hash_bytes(&restored), snap.assets[0].0.id);
    }

    #[test]
    fn repeated_save_reopen_is_stable() {
        let store = AssetStore::new_session().unwrap();
        let snap = sample(&store);
        let out = store.dir().join("deck.beamerx");
        let mut doc = snap.doc.clone();
        for _ in 0..10 {
            save_package(
                &out,
                &SaveSnapshot {
                    doc: doc.clone(),
                    assets: snap.assets.clone(),
                },
            )
            .unwrap();
            doc = load_package(&out, store.dir()).unwrap().doc;
        }
        assert_eq!(doc, snap.doc);
        // No temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(store.dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".saving-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn failed_save_keeps_previous_file() {
        let store = AssetStore::new_session().unwrap();
        let snap = sample(&store);
        let out = store.dir().join("deck.beamerx");
        save_package(&out, &snap).unwrap();
        let before = fs::read(&out).unwrap();
        // Simulate a failure mid-save: an asset file disappeared.
        let mut broken = snap.clone();
        broken.assets[0].1 = store.dir().join("does-not-exist.png");
        broken.doc.meta.title = "changed".into();
        assert!(save_package(&out, &broken).is_err());
        assert_eq!(fs::read(&out).unwrap(), before);
        assert!(
            fs::read_dir(store.dir())
                .unwrap()
                .all(|e| { !e.unwrap().file_name().to_string_lossy().contains(".saving-") })
        );
    }

    #[test]
    fn rejects_garbage_and_future_versions() {
        let store = AssetStore::new_session().unwrap();
        let p = store.dir().join("garbage.beamerx");
        fs::write(&p, b"definitely not a zip").unwrap();
        assert!(load_package(&p, store.dir()).is_err());

        let snap = sample(&store);
        let mut v = serde_json::to_value(Manifest {
            format: FORMAT_NAME.into(),
            format_version: FORMAT_VERSION + 1,
            app_version: "9.9.9".into(),
            document: snap.doc.clone(),
            assets: vec![],
        })
        .unwrap();
        let e = parse_manifest(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert!(format!("{e:#}").contains("newer"), "{e:#}");
        v["format"] = "other".into();
        assert!(parse_manifest(&serde_json::to_vec(&v).unwrap()).is_err());
    }

    #[test]
    fn corrupt_asset_is_reported_not_fatal() {
        let store = AssetStore::new_session().unwrap();
        let mut snap = sample(&store);
        // Point the manifest at bytes that do not match the hash.
        let bogus = store.dir().join("bogus.png");
        fs::write(&bogus, vec![0u8; snap.assets[0].0.bytes as usize]).unwrap();
        snap.assets[0].1 = bogus;
        let out = store.dir().join("deck.beamerx");
        save_package(&out, &snap).unwrap();
        let other = AssetStore::new_session().unwrap();
        let loaded = load_package(&out, other.dir()).unwrap();
        assert!(loaded.assets.is_empty());
        assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
    }

    #[test]
    fn v2_bullets_become_paragraph_lists() {
        let snap = SaveSnapshot {
            doc: Document::new(),
            assets: vec![],
        };
        let mut v = serde_json::to_value(Manifest {
            format: FORMAT_NAME.into(),
            format_version: 2,
            app_version: "0.1.0".into(),
            document: snap.doc,
            assets: vec![],
        })
        .unwrap();
        v["document"]["slides"][0]["objects"] = serde_json::json!([{
            "id": 900, "rect": {"x": 0.0, "y": 0.0, "w": 100.0, "h": 20.0}, "type": "text",
            "text": "one\ntwo\nthree", "font_size": 18.0, "color": "#000000", "bullets": true
        }]);
        let (doc, _) = parse_manifest(&serde_json::to_vec(&v).unwrap()).unwrap();
        match &doc.slides[0].objects[0].kind {
            ObjectKind::Text(t) => {
                assert_eq!(t.paras.len(), 3);
                assert!(t.paras.iter().all(|p| p.list == ListKind::Bullet && p.level == 0));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn frozen_v1_fixture_still_loads() {
        let bytes = include_bytes!("../tests/fixtures/project_v1.json");
        let (doc, assets) = parse_manifest(bytes).unwrap();
        assert_eq!(doc.slides.len(), 2);
        assert_eq!(doc.meta.title, "Fixture deck");
        assert_eq!(assets.len(), 1);
        let kinds: Vec<&str> = doc.slides[0].objects.iter().map(|o| o.kind.label()).collect();
        assert_eq!(kinds, ["Text", "Text", "Image", "Rectangle", "Arrow"]);
        // The v1 equation became a text box with display-style inline math.
        let eq = &doc.slides[0].objects[1];
        match &eq.kind {
            ObjectKind::Text(t) => {
                assert_eq!(t.text, r"$\displaystyle f(x) = \int_{\R} e^{-x^2}\,dx$");
                assert_eq!(t.color, Color::rgb(0x1f, 0x5f, 0xa8));
                assert_eq!(t.font_size, 20.0);
            }
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!((eq.rect.x, eq.rect.y), (20.0, 70.0));
        assert!(eq.rect.w > 120.0);
    }
}
