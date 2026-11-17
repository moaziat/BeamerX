//! Rasterize PDF pages (equation previews produced by TeX) into 8-bit
//! coverage masks using macOS CoreGraphics.
//!
//! # Safety
//!
//! This is the only module that calls into CoreGraphics. Every object
//! created here is released before returning (RAII guards below); pixel
//! buffers are owned by Rust and outlive the bitmap context that writes into
//! them. Inputs are validated (null checks, size limits) before use.
//! CoreGraphics objects are created and destroyed on the calling thread and
//! are never shared between threads.

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

#[cfg(not(target_os = "macos"))]
pub fn page_sizes(_pdf: &[u8]) -> Result<Vec<(f32, f32)>> {
    bail!("equation preview rasterization is only implemented on macOS")
}

#[cfg(not(target_os = "macos"))]
pub fn rasterize_alpha(_pdf: &[u8], _index: usize, _scale: f32) -> Result<AlphaMask> {
    bail!("equation preview rasterization is only implemented on macOS")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

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
}
