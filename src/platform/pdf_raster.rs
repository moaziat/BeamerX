//! Rasterize PDF pages (equation previews produced by TeX) into 8-bit
//! coverage masks. macOS uses CoreGraphics; Linux uses the CPU-only Hayro
//! renderer and therefore needs no external PDF utility.
//!
//! # Safety
//!
//! On macOS, this is the only module that calls into CoreGraphics. Every object
//! created here is released before returning (RAII guards below); pixel
//! buffers are owned by Rust and outlive the bitmap context that writes into
//! them. Inputs are validated (null checks, size limits) before use.
//! CoreGraphics objects are created and destroyed on the calling thread and
//! are never shared between threads. Hayro's safe Rust API is used on Linux.

use anyhow::{Result, bail};

/// Largest raster side we produce (pixels).
pub const MAX_RASTER_SIDE: u32 = 8192;

/// 8-bit coverage mask: 0 = transparent, 255 = fully inked.
pub struct AlphaMask {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

#[cfg(target_os = "macos")]
mod cg {
    #![allow(non_snake_case)]
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct CGRect {
        pub origin: CGPoint,
        pub size: CGSize,
    }

    pub const K_CGPDF_CROP_BOX: i32 = 1;
    pub const K_CG_IMAGE_ALPHA_NONE: u32 = 0;
    pub const K_CG_INTERPOLATION_HIGH: i32 = 3;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
        pub fn CFRelease(cf: *const c_void);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGDataProviderCreateWithCFData(data: *const c_void) -> *mut c_void;
        pub fn CGDataProviderRelease(provider: *mut c_void);
        pub fn CGPDFDocumentCreateWithProvider(provider: *mut c_void) -> *mut c_void;
        pub fn CGPDFDocumentRelease(doc: *mut c_void);
        pub fn CGPDFDocumentGetNumberOfPages(doc: *mut c_void) -> usize;
        pub fn CGPDFDocumentGetPage(doc: *mut c_void, page_number: usize) -> *mut c_void;
        pub fn CGPDFPageGetBoxRect(page: *mut c_void, box_type: i32) -> CGRect;
        pub fn CGColorSpaceCreateDeviceGray() -> *mut c_void;
        pub fn CGColorSpaceRelease(space: *mut c_void);
        pub fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *mut c_void,
            bitmap_info: u32,
        ) -> *mut c_void;
        pub fn CGContextRelease(c: *mut c_void);
        pub fn CGContextSetGrayFillColor(c: *mut c_void, gray: f64, alpha: f64);
        pub fn CGContextFillRect(c: *mut c_void, rect: CGRect);
        pub fn CGContextScaleCTM(c: *mut c_void, sx: f64, sy: f64);
        pub fn CGContextTranslateCTM(c: *mut c_void, tx: f64, ty: f64);
        pub fn CGContextDrawPDFPage(c: *mut c_void, page: *mut c_void);
        pub fn CGContextSetInterpolationQuality(c: *mut c_void, quality: i32);
    }
}

#[cfg(target_os = "macos")]
struct PdfDocument {
    data: *const std::ffi::c_void,
    provider: *mut std::ffi::c_void,
    doc: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
impl PdfDocument {
    fn open(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > isize::MAX as usize {
            bail!("empty PDF");
        }
        // SAFETY: `bytes` is a valid slice; CFDataCreate copies it.
        let data = unsafe { cg::CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize) };
        if data.is_null() {
            bail!("CFDataCreate failed");
        }
        let mut me = PdfDocument {
            data,
            provider: std::ptr::null_mut(),
            doc: std::ptr::null_mut(),
        };
        // SAFETY: `data` is a valid CFData; the provider retains it.
        me.provider = unsafe { cg::CGDataProviderCreateWithCFData(me.data) };
        if me.provider.is_null() {
            bail!("CGDataProviderCreateWithCFData failed");
        }
        // SAFETY: `provider` is valid. Returns null for malformed PDFs.
        me.doc = unsafe { cg::CGPDFDocumentCreateWithProvider(me.provider) };
        if me.doc.is_null() {
            bail!("invalid PDF");
        }
        Ok(me)
    }

    fn page_count(&self) -> usize {
        // SAFETY: `doc` is a valid CGPDFDocument.
        unsafe { cg::CGPDFDocumentGetNumberOfPages(self.doc) }
    }

    /// Borrowed page pointer (owned by the document), 0-based index.
    fn page(&self, index: usize) -> Result<*mut std::ffi::c_void> {
        if index >= self.page_count() {
            bail!("PDF page {index} out of range");
        }
        // SAFETY: valid doc; page numbers are 1-based and checked above.
        let p = unsafe { cg::CGPDFDocumentGetPage(self.doc, index + 1) };
        if p.is_null() {
            bail!("missing PDF page");
        }
        Ok(p)
    }

    fn crop_box(&self, page: *mut std::ffi::c_void) -> cg::CGRect {
        // SAFETY: `page` was obtained from this live document.
        unsafe { cg::CGPDFPageGetBoxRect(page, cg::K_CGPDF_CROP_BOX) }
    }
}

#[cfg(target_os = "macos")]
impl Drop for PdfDocument {
    fn drop(&mut self) {
        // SAFETY: each pointer is either null or an object we created and
        // have not released yet.
        unsafe {
            if !self.doc.is_null() {
                cg::CGPDFDocumentRelease(self.doc);
            }
            if !self.provider.is_null() {
                cg::CGDataProviderRelease(self.provider);
            }
            if !self.data.is_null() {
                cg::CFRelease(self.data);
            }
        }
    }
}

/// Page sizes (width, height) in bp for every page of `pdf`.
#[cfg(target_os = "macos")]
pub fn page_sizes(pdf: &[u8]) -> Result<Vec<(f32, f32)>> {
    let doc = PdfDocument::open(pdf)?;
    (0..doc.page_count())
        .map(|i| {
            let r = doc.crop_box(doc.page(i)?);
            Ok((r.size.width as f32, r.size.height as f32))
        })
        .collect()
}

/// Rasterize page `index` (0-based) at `scale` pixels per bp into a
/// coverage mask. Ink is assumed to be dark on a transparent page (TeX
/// output); colour information is discarded.
#[cfg(target_os = "macos")]
pub fn rasterize_alpha(pdf: &[u8], index: usize, scale: f32) -> Result<AlphaMask> {
    if !(scale.is_finite() && scale > 0.0) {
        bail!("invalid raster scale");
    }
    let doc = PdfDocument::open(pdf)?;
    let page = doc.page(index)?;
    let b = doc.crop_box(page);
    let w = (b.size.width * f64::from(scale)).ceil().max(1.0);
    let h = (b.size.height * f64::from(scale)).ceil().max(1.0);
    if w > f64::from(MAX_RASTER_SIDE) || h > f64::from(MAX_RASTER_SIDE) {
        bail!("equation raster too large ({w}×{h} px)");
    }
    let (w, h) = (w as usize, h as usize);
    let stride = (w + 15) & !15;
    let mut buf = vec![255u8; stride * h];

    // SAFETY: `buf` has stride*h bytes and outlives `ctx` (released below
    // before `buf` is read or dropped). The colour space is released after
    // the context retains it.
    unsafe {
        let space = cg::CGColorSpaceCreateDeviceGray();
        if space.is_null() {
            bail!("CGColorSpaceCreateDeviceGray failed");
        }
        let ctx = cg::CGBitmapContextCreate(
            buf.as_mut_ptr().cast(),
            w,
            h,
            8,
            stride,
            space,
            cg::K_CG_IMAGE_ALPHA_NONE,
        );
        cg::CGColorSpaceRelease(space);
        if ctx.is_null() {
            bail!("CGBitmapContextCreate failed");
        }
        cg::CGContextSetGrayFillColor(ctx, 1.0, 1.0);
        cg::CGContextFillRect(
            ctx,
            cg::CGRect {
                origin: cg::CGPoint { x: 0.0, y: 0.0 },
                size: cg::CGSize {
                    width: w as f64,
                    height: h as f64,
                },
            },
        );
        cg::CGContextSetInterpolationQuality(ctx, cg::K_CG_INTERPOLATION_HIGH);
        cg::CGContextScaleCTM(ctx, f64::from(scale), f64::from(scale));
        cg::CGContextTranslateCTM(ctx, -b.origin.x, -b.origin.y);
        cg::CGContextDrawPDFPage(ctx, page);
        cg::CGContextRelease(ctx);
    }

    // Bitmap memory starts at the top row. Convert gray (white paper, dark
    // ink) to coverage.
    let mut alpha = Vec::with_capacity(w * h);
    for row in buf.chunks_exact(stride) {
        alpha.extend(row[..w].iter().map(|&g| 255 - g));
    }
    Ok(AlphaMask {
        width: w as u32,
        height: h as u32,
        alpha,
    })
}

#[cfg(target_os = "linux")]
fn hayro_pdf(pdf: &[u8]) -> Result<hayro::hayro_syntax::Pdf> {
    if pdf.is_empty() {
        bail!("empty PDF");
    }
    hayro::hayro_syntax::Pdf::new(pdf.to_vec()).map_err(|error| anyhow::anyhow!("invalid PDF: {error:?}"))
}

#[cfg(target_os = "linux")]
fn hayro_page_sizes(pdf: &[u8]) -> Result<Vec<(f32, f32)>> {
    let pdf = hayro_pdf(pdf)?;
    let sizes = pdf
        .pages()
        .iter()
        .map(|page| page.render_dimensions())
        .collect::<Vec<_>>();
    if sizes.is_empty() {
        bail!("PDF has no pages");
    }
    Ok(sizes)
}

/// Rasterize through Hayro on Linux. Rendering stays in-process so the
/// installed application does not depend on `pdftocairo`, PDFium, or another
/// system PDF library.
#[cfg(target_os = "linux")]
fn hayro_rasterize_alpha(pdf: &[u8], index: usize, scale: f32) -> Result<AlphaMask> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

    if !(scale.is_finite() && scale > 0.0) {
        bail!("invalid raster scale");
    }
    let pdf = hayro_pdf(pdf)?;
    let pages = pdf.pages();
    let page = pages
        .get(index)
        .ok_or_else(|| anyhow::anyhow!("PDF page {index} out of range"))?;
    let (page_width, page_height) = page.render_dimensions();
    if !(page_width.is_finite() && page_height.is_finite() && page_width > 0.0 && page_height > 0.0) {
        bail!("invalid PDF page size ({page_width}×{page_height} bp)");
    }
    let width = (page_width * scale).ceil().max(1.0);
    let height = (page_height * scale).ceil().max(1.0);
    if width > MAX_RASTER_SIDE as f32 || height > MAX_RASTER_SIDE as f32 {
        bail!("equation raster too large ({width}×{height} px)");
    }

    let pixmap = render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let width = u32::from(pixmap.width());
    let height = u32::from(pixmap.height());
    if width == 0 || height == 0 {
        bail!("PDF renderer produced an empty equation preview");
    }

    // Hayro returns opaque premultiplied RGBA on the requested white
    // background. Equations are dark-on-white; turn perceived brightness
    // into coverage while intentionally discarding colour, like the macOS
    // path. Integer Rec. 709 weights preserve anti-aliased edge smoothness.
    let rgba = pixmap.data_as_u8_slice();
    debug_assert_eq!(rgba.len() % 4, 0);
    let alpha = (0..rgba.len())
        .step_by(4)
        .map(|i| {
            let luminance =
                (54 * u32::from(rgba[i]) + 183 * u32::from(rgba[i + 1]) + 19 * u32::from(rgba[i + 2]) + 128) >> 8;
            (255 - luminance) as u8
        })
        .collect();
    Ok(AlphaMask { width, height, alpha })
}

#[cfg(target_os = "linux")]
pub fn page_sizes(pdf: &[u8]) -> Result<Vec<(f32, f32)>> {
    hayro_page_sizes(pdf)
}

#[cfg(target_os = "linux")]
pub fn rasterize_alpha(pdf: &[u8], index: usize, scale: f32) -> Result<AlphaMask> {
    hayro_rasterize_alpha(pdf, index, scale)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn page_sizes(_pdf: &[u8]) -> Result<Vec<(f32, f32)>> {
    bail!("equation preview rasterization is not implemented on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn rasterize_alpha(_pdf: &[u8], _index: usize, _scale: f32) -> Result<AlphaMask> {
    bail!("equation preview rasterization is not implemented on this platform")
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::sync::atomic::AtomicBool;

    /// Minimal hand-written one-page PDF (100×50 bp) with a filled black
    /// rectangle covering the top-left quadrant.
    pub fn tiny_pdf() -> Vec<u8> {
        let content = b"0 0 0 rg 0 25 50 25 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                content.len(),
                std::str::from_utf8(content).unwrap()
            ),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for off in offsets {
            out.extend(format!("{off:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .bytes(),
        );
        out
    }

    #[test]
    fn page_size_and_orientation() {
        let pdf = tiny_pdf();
        assert_eq!(page_sizes(&pdf).unwrap(), vec![(100.0, 50.0)]);
        let m = rasterize_alpha(&pdf, 0, 2.0).unwrap();
        assert_eq!((m.width, m.height), (200, 100));
        let at = |x: u32, y: u32| m.alpha[(y * m.width + x) as usize];
        // Ink is in the top-left quadrant (PDF y-up is converted to rows top-down).
        assert_eq!(at(10, 10), 255);
        assert_eq!(at(150, 10), 0);
        assert_eq!(at(10, 90), 0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(page_sizes(b"not a pdf").is_err());
        assert!(rasterize_alpha(&tiny_pdf(), 3, 1.0).is_err());
        assert!(rasterize_alpha(&tiny_pdf(), 0, 1000.0).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hayro_backend_size_orientation_and_errors() {
        let pdf = tiny_pdf();
        assert_eq!(hayro_page_sizes(&pdf).unwrap(), vec![(100.0, 50.0)]);
        let m = hayro_rasterize_alpha(&pdf, 0, 2.0).unwrap();
        assert_eq!((m.width, m.height), (200, 100));
        let at = |x: u32, y: u32| m.alpha[(y * m.width + x) as usize];
        assert_eq!(at(10, 10), 255);
        assert_eq!(at(150, 10), 0);
        assert_eq!(at(10, 90), 0);
        assert!(hayro_page_sizes(b"not a pdf").is_err());
        assert!(hayro_rasterize_alpha(&pdf, 3, 1.0).is_err());
        assert!(hayro_rasterize_alpha(&pdf, 0, 1000.0).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hayro_backend_renders_real_latex_output_when_available() {
        let Some(toolchain) = crate::latex::Toolchain::detect() else {
            return;
        };
        let session = crate::assets::AssetStore::new_session().unwrap();
        let tex = r#"\documentclass{article}
\usepackage[active,tightpage]{preview}
\begin{document}
\begin{preview}
$\displaystyle \sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}$
\end{preview}
\end{document}
"#;
        std::fs::write(session.dir().join("equation.tex"), tex).unwrap();
        let path = toolchain
            .compile(
                session.dir(),
                "equation.tex",
                &crate::latex::CompileOptions::preview(),
                &AtomicBool::new(false),
            )
            .unwrap();
        let pdf = std::fs::read(path).unwrap();
        let sizes = hayro_page_sizes(&pdf).unwrap();
        assert_eq!(sizes.len(), 1);
        assert!(
            sizes[0].0 > 20.0 && sizes[0].1 > 10.0,
            "unexpected equation size: {:?}",
            sizes[0]
        );
        let mask = hayro_rasterize_alpha(&pdf, 0, 2.0).unwrap();
        let inked = mask.alpha.iter().filter(|&&alpha| alpha > 32).count();
        assert!(inked > 100, "equation rendered blank ({inked} inked pixels)");
    }
}
