//! Reproduce slides 73 and 74 of `presentation_template/*.pptx` in BeamerX.
//!
//! Positions and sizes come from the PowerPoint XML (960 × 540 pt slide),
//! scaled to Beamer's 16:9 page; equations (Office Math) are rewritten as
//! LaTeX inside text boxes. Writes a `.beamerx` project, the Beamer folder,
//! the PDF and PNG renders of each page.
//!
//! `cargo run --example template_73_74 -- <pptx> <output-dir>`

use beamerx::assets::AssetStore;
use beamerx::document::*;
use beamerx::latex::Toolchain;
use beamerx::persistence::{SaveSnapshot, save_package};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

/// PowerPoint points → slide-space bp.
const K: f32 = SLIDE_W / 960.0;
/// PowerPoint's default text-box insets (pt).
const INSET_X: f32 = 7.2;
const INSET_Y: f32 = 3.6;
const TITLE_RED: Color = Color::rgb(0xF4, 0x0E, 0x53);
const ARROW_TEAL: Color = Color::rgb(0x10, 0x48, 0x62);
// Key equations are coloured with standard colours of BeamerX's colour
// picker (the original slides set them in black).
const EQ_BLUE: Color = Color::rgb(0, 112, 192);
const EQ_PURPLE: Color = Color::rgb(112, 48, 160);
const EQ_RED: Color = Color::rgb(192, 0, 0);
const EQ_GREEN: Color = Color::rgb(0, 176, 80);

struct Deck {
    doc: Document,
    objects: Vec<Object>,
}

impl Deck {
    fn push(&mut self, rect: Rect, kind: ObjectKind) {
        let id = self.doc.alloc_object_id();
        self.objects.push(Object { id, rect, kind });
    }

    /// A PowerPoint text box at (x, y, w) pt with `size` pt text.
    fn text(&mut self, x: f32, y: f32, w: f32, size: f32, align: TextAlign, text: &str) -> &mut TextBox {
        let rect = Rect::new(
            (x + INSET_X) * K,
            (y + INSET_Y) * K,
            (w - 2.0 * INSET_X) * K,
            size * K * 1.2,
        );
        self.push(
            rect,
            ObjectKind::Text(TextBox {
                text: text.into(),
                font_size: size * K,
                align,
                ..Default::default()
            }),
        );
        match &mut self.objects.last_mut().expect("pushed").kind {
            ObjectKind::Text(t) => t,
            _ => unreachable!(),
        }
    }

    /// An Office Math box rewritten as display-style LaTeX. Computer Modern
    /// math sets about 13 % wider than Cambria Math at the same size, so the
    /// size is scaled to keep each formula inside its original box.
    fn math(&mut self, x: f32, y: f32, w: f32, size: f32, latex: &str) -> &mut TextBox {
        self.math_fit(x, y, w, size, 0.87, latex)
    }

    /// As [`Deck::math`] with an explicit width correction for formulas whose
    /// glyph mix differs most between the two fonts.
    fn math_fit(&mut self, x: f32, y: f32, w: f32, size: f32, factor: f32, latex: &str) -> &mut TextBox {
        self.text(
            x,
            y,
            w + 60.0,
            size * factor,
            TextAlign::Left,
            &format!("$\\displaystyle {latex}$"),
        )
    }

    fn block_arrow(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let mut s = Shape::new(ShapeKind::BlockArrow);
        s.fill = None;
        s.stroke = ARROW_TEAL;
        s.stroke_width = 0.5;
        self.push(Rect::new(x * K, y * K, w * K, h * K), ObjectKind::Shape(s));
    }

    fn title(&mut self) {
        // Bottom-anchored two-line title in the original: first baseline ≈ 33 pt.
        let size = 32.0 * K;
        let rect = Rect::new(
            (5.5 + INSET_X) * K,
            33.0 * K - size * typeset::FIRST_BASELINE,
            (954.5 - 2.0 * INSET_X) * K,
            size * 2.4,
        );
        self.push(
            rect,
            ObjectKind::Text(TextBox {
                text: "Revesting Rotational Transform: An Individual Particle Perspective".into(),
                font_size: size / ParaStyle::Title.size_factor(),
                color: TITLE_RED,
                paras: vec![ParaFmt {
                    style: ParaStyle::Title,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
    }

    fn finish_slide(&mut self) {
        let id = self.doc.alloc_slide_id();
        let objects = std::mem::take(&mut self.objects);
        self.doc.slides.push(Slide {
            id,
            objects,
            background: Color::WHITE,
        });
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let pptx = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "presentation_template/HBE_MHD_AlNour_Winter_School_final.pptx".into()),
    );
    let out = PathBuf::from(args.next().unwrap_or_else(|| "presentation_template".into()));
    std::fs::create_dir_all(&out)?;

    // The ι(ρ) plot on slide 73, straight from the PowerPoint package.
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&pptx)?)?;
    let mut plot = Vec::new();
    zip.by_name("ppt/media/image304.png")?.read_to_end(&mut plot)?;
    let store = AssetStore::new_session()?;
    let plot_meta = AssetStore::ingest_bytes(store.dir(), &plot, "iota_profile.png")?;

    let mut doc = Document::new();
    doc.meta.title = "Revisiting Rotational Transform: An Individual Particle Perspective".into();
    doc.meta.author = "HBE MHD — AlNour Winter School".into();
    doc.slides.clear();
    let mut d = Deck {
        doc,
        objects: Vec::new(),
    };

    // ---------------------------------------------------------------- slide 73
    d.title();
    d.push(
        Rect::new(644.0 * K, 53.8 * K, 303.3 * K, 304.3 * K),
        ObjectKind::Image(ImageRef {
            asset: plot_meta.id.clone(),
        }),
    );
    d.math(695.3, 59.9, 243.6, 20.0, r"QA,\ R_0 = 5.5,\ A = 4, a = 1.3");
    d.text(
        94.3,
        81.6,
        460.9,
        20.0,
        TextAlign::Center,
        "From gyro-kinetic equations of motion, the main drift an energetic particle exhibit",
    );
    d.math(
        40.7,
        154.8,
        509.3,
        22.0,
        r"\boldsymbol{v}_d = -\frac{m}{eB}\left(\frac{v_\perp^2}{2} + v_{para}^2\right)\frac{\nabla B \times \boldsymbol{B}}{B^2} \to v_d \sim \frac{v_{th}^2}{\Omega_c R} \sim \frac{\rho_L v_{th}}{R}",
    ).color = EQ_BLUE;
    {
        let text = "To the first order, one can assume particles are moving along field lines: v//B $\\rightarrow$ characteristic poloidal time";
        let t = d.text(64.9, 224.6, 532.0, 20.0, TextAlign::Center, text);
        // Bold "v" and "B" as in the original.
        let chars: Vec<char> = t.text.chars().collect();
        let v = chars.iter().position(|c| *c == '/').expect("v//B") - 1;
        t.format_range(v, v + 1, |f| f.bold = Some(true));
        t.format_range(v + 3, v + 4, |f| f.bold = Some(true));
    }
    d.math_fit(
        27.2,
        289.0,
        335.9,
        18.0,
        0.8,
        r"dl \times B = 0 \to \frac{dl_\theta/dt}{B_\theta} = \frac{dl_\zeta/dt}{B_\zeta} \to \frac{v_\theta}{v_\zeta} = \frac{B_\theta}{B_\zeta}",
    );
    // The original arrow starts 5 pt after this equation; Computer Modern
    // sets it a little wider than Cambria Math, so the arrow sits in the gap
    // between the equation and τ_pol instead (no overlap in editor or PDF).
    d.block_arrow(389.0, 307.1, 62.0, 12.2);
    d.math(
        457.6,
        289.6,
        169.9,
        22.0,
        r"\tau_{pol} = \frac{2\pi r}{v_\zeta (B_\theta/B_\zeta)}",
    )
    .color = EQ_PURPLE;
    d.text(
        24.9,
        348.6,
        592.9,
        20.0,
        TextAlign::Center,
        "From the definition of rotational transform, the first order approximation can be written as",
    );
    d.math(
        27.2,
        412.6,
        230.9,
        22.0,
        r"\frac{B_\theta}{B_\zeta} = \frac{r\iota}{R} \to \tau_{pol} \approx \frac{2\pi R}{v_{\mathrm{para}}\,\iota}",
    );
    d.block_arrow(272.4, 435.4, 75.9, 12.2);
    d.math(
        371.0,
        411.6,
        285.0,
        25.0,
        r"\Delta_d \sim v_d \tau_{pol} \sim \frac{\pi \rho_L v_{th}}{\iota\, v_{para}} \sim \frac{\pi\rho_L}{\iota}",
    )
    .color = EQ_RED;
    d.math(737.4, 399.2, 209.8, 18.0, r"\iota = 0.02 + fusion - \alpha");
    d.math(
        744.8,
        428.3,
        195.2,
        18.0,
        r"\Delta_d \sim \frac{0.05\pi}{0.02} \sim 7\ meter",
    )
    .color = EQ_GREEN;
    d.finish_slide();

    // ---------------------------------------------------------------- slide 74
    d.title();
    d.text(
        28.1,
        101.3,
        887.0,
        24.0,
        TextAlign::Left,
        "What the first-order approximation and low iota values reveal about trapped particles",
    );
    d.math(
        101.0,
        172.0,
        758.1,
        24.0,
        r"E = \mu B_b = \frac{1}{2} m v_{para}^2 + \mu B_{min} \to v_{para}^2 = \frac{2\mu}{m}\left(B_b - B_{min}\right) \sim \frac{2\mu\epsilon B}{m} \sim \epsilon v_{th}^2",
    ).color = EQ_BLUE;
    d.text(
        28.1,
        243.2,
        843.8,
        24.0,
        TextAlign::Left,
        "Following the same logic as the previous slide, a single drift step can be estimated as",
    );
    d.math(
        329.1,
        320.4,
        285.0,
        25.0,
        r"\Delta_d \sim v_d \tau_{pol} \sim \frac{\pi \rho_L v_{th}}{\iota\, v_{para}} \sim \frac{\pi\rho_L}{\iota\sqrt{\epsilon}}",
    ).color = EQ_PURPLE;
    d.math(
        15.3,
        406.5,
        430.4,
        20.0,
        r"\iota = 0.02,\ \epsilon = 0.32\ (ITER-like),\ fusion-\alpha",
    );
    d.block_arrow(426.5, 417.7, 75.9, 12.2);
    d.math(
        588.0,
        388.7,
        296.1,
        22.0,
        r"\Delta_d \sim \frac{0.05\pi}{0.02 \times 0.32} \sim 22\ meter",
    )
    .color = EQ_RED;
    d.text(
        764.0,
        435.3,
        196.0,
        15.0,
        TextAlign::Center,
        "Three times larger than that for passing particles",
    );
    {
        let mut box_ = Shape::new(ShapeKind::RoundedRectangle);
        box_.fill = Some(Color::rgb(0xEB, 0xF4, 0xEC));
        box_.stroke = Color::rgb(0x2F, 0x5D, 0x3A);
        box_.stroke_width = 0.5;
        d.push(
            Rect::new(93.0 * K, 493.0 * K, 773.0 * K, 42.0 * K),
            ObjectKind::Shape(box_),
        );
        d.text(
            93.0,
            497.0,
            773.0,
            24.0,
            TextAlign::Center,
            "Small rotational transform values correspond to poor confinement",
        );
    }
    d.finish_slide();

    let mut doc = d.doc;
    doc.validate_and_repair().map_err(anyhow::Error::msg)?;
    let path = store.path_of(&plot_meta);
    let snap = SaveSnapshot {
        doc,
        assets: vec![(plot_meta, path)],
    };
    let project = out.join("slides_73_74.beamerx");
    save_package(&project, &snap)?;
    beamerx::export::export_tex_package(&out.join("slides_73_74-beamer"), &snap)?;
    let tc = Toolchain::detect().ok_or_else(|| anyhow::anyhow!("no LaTeX engine"))?;
    let pdf_path = out.join("slides_73_74.pdf");
    beamerx::export::export_pdf(&pdf_path, &snap, &tc, &AtomicBool::new(false))?;
    println!("wrote {} and {}", project.display(), pdf_path.display());
    Ok(())
}
