//! Build a small showcase deck, save it as `.beamerx`, export the Beamer
//! folder and PDF, and rasterize each PDF page to PNG for inspection.
//!
//! `cargo run --example showcase -- <output-dir>`

use beamerx::assets::AssetStore;
use beamerx::document::*;
use beamerx::latex::Toolchain;
use beamerx::persistence::{SaveSnapshot, save_package};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "showcase-out".into()));
    std::fs::create_dir_all(&out)?;
    let store = AssetStore::new_session()?;
    let img = image::RgbImage::from_fn(320, 200, |x, y| {
        let v = ((x as f32 / 20.0).sin() * (y as f32 / 15.0).cos() * 120.0 + 128.0) as u8;
        image::Rgb([v, 90, 255 - v])
    });
    let mut bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)?;
    let meta = AssetStore::ingest_bytes(store.dir(), &bytes, "field.png")?;

    let mut doc = Document::new();
    doc.meta.title = "BeamerX showcase".into();
    doc.meta.author = "BeamerX".into();
    doc.meta.preamble = "\\newcommand{\\R}{\\mathbb{R}}".into();
    let s1 = doc.slides[0].id;
    let mut objs = vec![];
    let mut next = || doc.alloc_object_id();
    objs.push(Object {
        id: next(),
        rect: Rect::new(24.0, 18.0, 405.0, 34.0),
        kind: ObjectKind::Text(TextBox {
            text: "Gaussian integrals in practice".into(),
            font_size: 24.0,
            bold: true,
            color: Color::ACCENT,
            ..Default::default()
        }),
    });
    objs.push(Object {
        id: next(),
        rect: Rect::new(24.0, 64.0, 200.0, 80.0),
        kind: ObjectKind::Text(TextBox {
            text: "Closed form for the Gaussian\nWorks in any dimension n\nUsed in statistics & physics".into(),
            font_size: 12.0,
            bullets: true,
            ..Default::default()
        }),
    });
    objs.push(Object {
        id: next(),
        rect: Rect::new(24.0, 150.0, 150.0, 30.0),
        kind: ObjectKind::Equation(Equation {
            source: r"\int_{\R} e^{-x^2}\,dx = \sqrt{\pi}".into(),
            font_size: 18.0,
            color: Color::BLACK,
        }),
    });
    objs.push(Object {
        id: next(),
        rect: Rect::new(24.0, 200.0, 150.0, 30.0),
        kind: ObjectKind::Equation(Equation {
            source: r"\int_{\R^n} e^{-\lVert x \rVert^2}\,dx = \pi^{n/2}".into(),
            font_size: 14.0,
            color: Color::rgb(160, 30, 30),
        }),
    });
    objs.push(Object {
        id: next(),
        rect: Rect::new(250.0, 64.0, 180.0, 112.5),
        kind: ObjectKind::Image(ImageRef { asset: meta.id.clone() }),
    });
    let mut arrow = Shape::new(ShapeKind::Arrow);
    let r = arrow.set_endpoints((200.0, 160.0), (255.0, 130.0));
    objs.push(Object {
        id: next(),
        rect: r,
        kind: ObjectKind::Shape(arrow),
    });
    objs.push(Object {
        id: next(),
        rect: Rect::new(250.0, 190.0, 180.0, 40.0),
        kind: ObjectKind::Shape(Shape::new(ShapeKind::Rectangle)),
    });
    objs.push(Object {
        id: next(),
        rect: Rect::new(258.0, 200.0, 164.0, 20.0),
        kind: ObjectKind::Text(TextBox {
            text: "Box with centred caption".into(),
            font_size: 12.0,
            align: TextAlign::Center,
            ..Default::default()
        }),
    });
    doc.slide_mut(s1).unwrap().objects = objs;

    let path = store.path_of(&meta);
    let snap = SaveSnapshot {
        doc,
        assets: vec![(meta, path)],
    };
    save_package(&out.join("showcase.beamerx"), &snap)?;
    beamerx::export::export_tex_package(&out.join("showcase-beamer"), &snap)?;
    let tc = Toolchain::detect().ok_or_else(|| anyhow::anyhow!("no LaTeX engine"))?;
    let pdf_path = out.join("showcase.pdf");
    beamerx::export::export_pdf(&pdf_path, &snap, &tc, &AtomicBool::new(false))?;
    let pdf = std::fs::read(&pdf_path)?;
    for (i, _) in beamerx::platform::pdf_raster::page_sizes(&pdf)?.iter().enumerate() {
        let m = beamerx::platform::pdf_raster::rasterize_alpha(&pdf, i, 3.0)?;
        // Render ink as dark on white for viewing.
        let gray = image::GrayImage::from_raw(m.width, m.height, m.alpha.iter().map(|a| 255 - a).collect()).unwrap();
        gray.save(out.join(format!("page-{}.png", i + 1)))?;
    }
    println!("wrote {}", out.display());
    Ok(())
}
