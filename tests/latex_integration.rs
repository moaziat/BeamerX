//! End-to-end checks with a real LaTeX engine. Skipped (with a message)
//! when no engine is installed; CI for releases must have one.

use beamerx::assets::AssetStore;
use beamerx::document::*;
#[allow(unused_imports)]
use beamerx::document::{ListKind, ParaStyle};
use beamerx::latex::Toolchain;
use beamerx::persistence::SaveSnapshot;
use beamerx::platform::pdf_raster;
use std::sync::atomic::AtomicBool;

fn toolchain() -> Option<Toolchain> {
    let tc = Toolchain::detect();
    if tc.is_none() {
        eprintln!("SKIPPED: no LaTeX engine found (install tectonic)");
    }
    tc
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 4) as u8, (y * 4) as u8, 128]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn sample_deck(store: &AssetStore) -> SaveSnapshot {
    let meta = AssetStore::ingest_bytes(store.dir(), &png(64, 48), "my plot (final).png").unwrap();
    let mut doc = Document::new();
    doc.meta.title = "Integration & tests: 100% {braces}".into();
    doc.meta.author = "A. Tester".into();
    doc.meta.preamble = "\\newcommand{\\R}{\\mathbb{R}}\n\\usetheme{default}".into();
    let s1 = doc.slides[0].id;
    let add = |doc: &mut Document, slide: SlideId, rect: Rect, kind: ObjectKind| {
        let id = doc.alloc_object_id();
        doc.slide_mut(slide).unwrap().objects.push(Object { id, rect, kind });
    };
    add(
        &mut doc,
        s1,
        Rect::new(20.0, 15.0, 400.0, 30.0),
        ObjectKind::Text(TextBox {
            text: "Special: \\ { } $ & # % _ ^ ~ < > | “quotes” – é ü ß π".into(),
            font_size: 14.0,
            bold: true,
            ..Default::default()
        }),
    );
    add(
        &mut doc,
        s1,
        Rect::new(20.0, 50.0, 200.0, 60.0),
        ObjectKind::Text({
            let mut t = TextBox {
                text: "Heading\nFirst bullet\n\nSecond bullet that is long enough to wrap onto another line\n[bracket] nested\nStep one\nStep two".into(),
                font_size: 11.0,
                italic: true,
                ..Default::default()
            };
            t.format_lines(0, 0, |p| p.style = ParaStyle::Heading1);
            t.format_lines(1, 4, |p| p.list = ListKind::Bullet);
            t.format_lines(4, 4, |p| p.level = 1);
            t.format_lines(5, 6, |p| p.list = ListKind::Number);
            // A bold word, a red word and a blue formula inside a sentence.
            t.format_range(1, 6, |f| f.bold = Some(true));
            t.format_range(8, 14, |f| f.color = Some(Color::rgb(200, 0, 0)));
            t
        }),
    );
    add(
        &mut doc,
        s1,
        Rect::new(250.0, 50.0, 180.0, 40.0),
        ObjectKind::Text(TextBox {
            text: "Centered\n[line two]".into(),
            align: TextAlign::Center,
            color: Color::rgba(200, 0, 0, 180),
            ..Default::default()
        }),
    );
    add(
        &mut doc,
        s1,
        Rect::new(20.0, 130.0, 260.0, 20.0),
        ObjectKind::Text(TextBox {
            text: "Gauss: $f(x) = \\int_{\\R} e^{-x^2}\\,dx % trailing comment$ and\n$$\\sum_{n=1}^\\infty \\frac{1}{n^2} = \\frac{\\pi^2}{6}$$ costs \\$5".into(),
            font_size: 14.0,
            color: Color::ACCENT,
            ..Default::default()
        }),
    );
    add(
        &mut doc,
        s1,
        Rect::new(20.0, 170.0, 100.0, 20.0),
        ObjectKind::Text(TextBox {
            // Broken math must not break the document.
            text: "broken $\\frac{a}{b$ here".into(),
            ..Default::default()
        }),
    );
    add(
        &mut doc,
        s1,
        Rect::new(300.0, 120.0, 128.0, 96.0),
        ObjectKind::Image(ImageRef { asset: meta.id.clone() }),
    );
    add(
        &mut doc,
        s1,
        Rect::new(300.0, 220.0, 0.0, 0.0),
        ObjectKind::Image(ImageRef {
            asset: AssetId("c".repeat(64)),
        }),
    );
    let mut arrow = Shape::new(ShapeKind::Arrow);
    let r = arrow.set_endpoints((150.0, 200.0), (280.0, 160.0));
    add(&mut doc, s1, r, ObjectKind::Shape(arrow));
    add(
        &mut doc,
        s1,
        Rect::new(150.0, 210.0, 60.0, 30.0),
        ObjectKind::Shape(Shape::new(ShapeKind::Ellipse)),
    );
    let mut rect = Shape::new(ShapeKind::Rectangle);
    rect.fill = Some(Color::rgba(255, 200, 0, 100));
    add(
        &mut doc,
        s1,
        Rect::new(220.0, 210.0, 60.0, 30.0),
        ObjectKind::Shape(rect),
    );
    // An empty second slide with a coloured background.
    let s2 = doc.alloc_slide_id();
    doc.slides.push(Slide {
        id: s2,
        objects: vec![],
        background: Color::rgb(20, 30, 60),
    });
    let path = store.path_of(&meta);
    SaveSnapshot {
        doc,
        assets: vec![(meta, path)],
    }
}

#[test]
fn exported_beamer_compiles_to_expected_pages() {
    let Some(tc) = toolchain() else { return };
    let store = AssetStore::new_session().unwrap();
    let snap = sample_deck(&store);

    // Folder export.
    let folder = store.dir().join("deck-beamer");
    beamerx::export::export_tex_package(&folder, &snap).unwrap();
    assert!(folder.join("main.tex").is_file());
    let imgs: Vec<_> = std::fs::read_dir(folder.join("images")).unwrap().collect();
    assert_eq!(imgs.len(), 1);
    // Re-export over the existing folder replaces it atomically.
    beamerx::export::export_tex_package(&folder, &snap).unwrap();

    // PDF export.
    let pdf_path = store.dir().join("deck.pdf");
    beamerx::export::export_pdf(&pdf_path, &snap, &tc, &AtomicBool::new(false)).unwrap();
    let pdf = std::fs::read(&pdf_path).unwrap();
    let pages = pdf_raster::page_sizes(&pdf).unwrap();
    assert_eq!(pages.len(), 2, "one PDF page per slide");
    for (w, h) in pages {
        assert!((w - SLIDE_W).abs() < 0.6 && (h - SLIDE_H).abs() < 0.6, "page {w}×{h}");
    }
    // Slide 2 background is dark; slide 1 is mostly white.
    let p2 = pdf_raster::rasterize_alpha(&pdf, 1, 0.25).unwrap();
    let ink2 = p2.alpha.iter().map(|&a| a as u64).sum::<u64>() / p2.alpha.len() as u64;
    assert!(ink2 > 150, "dark background expected, mean ink {ink2}");
    let p1 = pdf_raster::rasterize_alpha(&pdf, 0, 0.25).unwrap();
    let ink1 = p1.alpha.iter().map(|&a| a as u64).sum::<u64>() / p1.alpha.len() as u64;
    assert!(ink1 < 60, "light slide expected, mean ink {ink1}");
}

#[test]
fn cancelled_pdf_export_leaves_no_output() {
    let Some(tc) = toolchain() else { return };
    let store = AssetStore::new_session().unwrap();
    let snap = sample_deck(&store);
    let out = store.dir().join("cancelled.pdf");
    let err = beamerx::export::export_pdf(&out, &snap, &tc, &AtomicBool::new(true)).unwrap_err();
    assert!(err.downcast_ref::<beamerx::latex::Cancelled>().is_some(), "{err:#}");
    assert!(!out.exists());
}

#[test]
fn equation_previews_typeset_in_batch_and_isolate_errors() {
    let Some(tc) = toolchain() else { return };
    let items = [
        (r"E = mc^2", 20.0, true),
        (r"\undefinedmacro{x}", 20.0, false),
        (r"\sum_{n=1}^{\infty} \frac{1}{n^2}", 20.0, true),
        (r"E = mc^2", 40.0, true),
        (r"\R^n", 12.0, false),
        (r"f(x)", 20.0, false),
    ];
    let res = beamerx::math::typeset_blocking(&items, "\\newcommand{\\R}{\\mathbb{R}}\n\\usetheme{Madrid}", &tc);
    let a = res[0].as_ref().expect("simple equation");
    assert!(
        res[1].as_ref().unwrap_err().contains("Undefined control sequence"),
        "{:?}",
        res[1]
    );
    let sum = res[2].as_ref().expect("display sum");
    let big = res[3].as_ref().expect("40bp");
    assert!(res[4].is_ok(), "user macro from preamble: {:?}", res[4]);
    // Display-style sums are taller than inline text; doubling the size
    // doubles the box (fix-cm gives exact scaling).
    assert!(sum.1 > a.1 * 1.5, "{sum:?} vs {a:?}");
    assert!(
        (big.0 / a.0 - 2.0).abs() < 0.02 && (big.1 / a.1 - 2.0).abs() < 0.02,
        "{big:?} vs {a:?}"
    );
    // Inline math reports its depth below the baseline: f(x) descends
    // (parentheses) by roughly a quarter of the font size.
    let fx = res[5].as_ref().expect("inline f(x)");
    assert!(fx.2 > 2.0 && fx.2 < 8.0, "depth of f(x) at 20pt: {fx:?}");
    assert!((fx.1 - fx.2) > 10.0, "height above baseline: {fx:?}");
}
