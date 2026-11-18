//! Beamer export.
//!
//! Every object is placed absolutely with TikZ (`remember picture, overlay`)
//! in slide-space bp coordinates, so geometry in the exported PDF matches the
//! editor exactly. Text is typeset by TeX in Latin Modern Sans (the font the
//! editor uses); math (`$…$` in text boxes) is typeset by TeX in both the editor preview and
//! the export using the same preamble, packages and font sizes.

use crate::assets::AssetMeta;
use crate::document::{
    AssetId, CharFmt, Color, Document, ListKind, Object, ObjectKind, ParaFmt, ParaStyle, Shape, ShapeKind, Slide,
    TextAlign, TextBox, typeset,
};
use crate::latex::{CompileOptions, Toolchain};
use crate::persistence::SaveSnapshot;
use crate::rich;
use anyhow::{Context as _, Result, bail};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub const MAIN_TEX: &str = "main.tex";

/// LaTeX packages shared by equation previews and the exported document.
/// `fix-cm` (loaded before the class) makes Computer Modern math scalable to
/// the exact requested size instead of the nearest design size.
pub const PRE_CLASS: &str = "\\RequirePackage{fix-cm}\n";
pub const MATH_PACKAGES: &str = "\\usepackage{amsmath,amssymb}\n";

/// Escape plain text for LaTeX (inside a TikZ node).
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '$' => out.push_str("\\$"),
            '&' => out.push_str("\\&"),
            '#' => out.push_str("\\#"),
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            '^' => out.push_str("\\textasciicircum{}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '<' => out.push_str("\\textless{}"),
            '>' => out.push_str("\\textgreater{}"),
            '|' => out.push_str("\\textbar{}"),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Make equation source safe to embed in `$…$`: no blank lines (paragraph
/// breaks are errors in math mode), no control characters, and a trailing
/// newline so a `%` comment cannot swallow the closing `$`.
pub fn normalize_math(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 1);
    let mut blank_run = false;
    for line in src.lines() {
        if line.trim().is_empty() {
            blank_run = true;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = blank_run;
        blank_run = false;
        out.extend(line.chars().filter(|c| !c.is_control() || *c == '\t'));
    }
    out.push('\n');
    out
}

/// Cheap structural check that keeps one broken equation from breaking the
/// whole document: braces must balance and some commands that end math mode
/// or the document are rejected.
pub fn math_problem(src: &str) -> Option<&'static str> {
    let mut depth: i64 = 0;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '%' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return Some("unbalanced braces");
                }
            }
            '$' => return Some("`$` is not allowed inside math"),
            _ => {}
        }
    }
    if depth != 0 {
        return Some("unbalanced braces");
    }
    for bad in ["\\end{document}", "\\begin{document}", "\\documentclass"] {
        if src.contains(bad) {
            return Some("document-level commands are not allowed in math");
        }
    }
    None
}

fn bp(v: f32) -> String {
    // Four decimals in bp is ~1.4 µm: more than enough, and stable text.
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

pub fn color_name(c: Color) -> String {
    format!("bx{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

fn opacity(c: Color) -> Option<String> {
    (c.a < 255).then(|| bp(f32::from(c.a) / 255.0))
}

fn font_cmd(size: f32) -> String {
    format!(
        "\\fontsize{{{}bp}}{{{}bp}}\\selectfont",
        bp(size),
        bp(size * typeset::LINE_SPREAD)
    )
}

/// Text box body: lines separated by `\\`, or a list for bullets.
/// LaTeX for one paragraph's content (text escaped, math passed through).
/// Switches for a character's format relative to the paragraph defaults.
fn fmt_switch(r: &crate::document::Resolved, base: &crate::document::Resolved) -> String {
    let mut sw = String::new();
    if r.bold != base.bold {
        sw.push_str(if r.bold { "\\bfseries" } else { "\\mdseries" });
    }
    if r.italic != base.italic {
        sw.push_str(if r.italic { "\\itshape" } else { "\\upshape" });
    }
    if r.color != base.color {
        let _ = write!(sw, "\\color{{{}}}", color_name(r.color));
    }
    sw
}

/// LaTeX for one paragraph's content: text escaped and grouped by character
/// format, math passed through.
fn paragraph_tex(t: &TextBox, chars: &[char], fmts: &[CharFmt], pf: ParaFmt, para: &rich::Paragraph) -> String {
    let base = t.resolve(CharFmt::default(), pf);
    let at = |i: usize| t.resolve(fmts.get(i).copied().unwrap_or_default(), pf);
    let math = |src: &std::ops::Range<usize>, display: bool, start: usize| {
        let s = rich::slice(chars, src);
        let body = match math_problem(&s) {
            None => crate::math::math_mode(&s, display),
            Some(why) => format!("\\textcolor{{red}}{{[math error: {}]}}", escape_text(why)),
        };
        let r = at(start);
        if r.color != base.color {
            format!("{{\\color{{{}}}{body}}}", color_name(r.color))
        } else {
            body
        }
    };
    match &para.kind {
        rich::ParaKind::Display { src } => {
            format!(
                "\\hspace*{{\\fill}}{}\\hspace*{{\\fill}}",
                math(src, true, para.range.start)
            )
        }
        rich::ParaKind::Normal(runs) => {
            let mut out = String::new();
            for r in runs {
                match &r.kind {
                    rich::RunKind::Text => {
                        // Split the run where the format changes.
                        let mut i = r.range.start;
                        while i < r.range.end {
                            let f = at(i);
                            let mut j = i + 1;
                            while j < r.range.end && at(j) == f {
                                j += 1;
                            }
                            let text = escape_text(&rich::unescape_text(&chars[i..j]));
                            let sw = fmt_switch(&f, &base);
                            if sw.is_empty() {
                                out.push_str(&text);
                            } else {
                                let _ = write!(out, "{{{sw}{{}}{text}}}");
                            }
                            i = j;
                        }
                    }
                    rich::RunKind::Math { src } => out.push_str(&math(src, false, r.range.start)),
                }
            }
            out
        }
    }
}

/// Text box body. Every line is its own TeX paragraph (so each can have its
/// own size and indentation) starting with `\strut`, matching the editor's
/// baseline convention. List labels hang in the indent.
fn text_body(t: &TextBox) -> String {
    let chars: Vec<char> = t.text.chars().collect();
    let paras = rich::paragraphs(&chars);
    let fmts = t.char_formats();
    let pfs = t.paras_normalized();
    let labels = crate::document::list_labels(&pfs);
    let unit = t.font_size * typeset::BULLET_INDENT;
    let mut out = String::from("\\parindent=0pt\\parskip=0pt\\relax ");
    for p in &paras {
        let pf = pfs.get(p.line).copied().unwrap_or_default();
        let size = t.font_size * pf.style.size_factor();
        let list = pf.list != ListKind::None;
        let indent = f32::from(pf.level) * unit + if list { unit } else { 0.0 };
        out.push('{');
        out.push_str(&font_cmd(size));
        if pf.style.bold() && !t.bold {
            out.push_str("\\bfseries");
        }
        if pf.style.italic() && !t.italic {
            out.push_str("\\itshape");
        }
        if indent > 0.0 {
            let _ = write!(out, "\\advance\\leftskip by {}bp\\relax", bp(indent));
        }
        out.push_str("\\strut ");
        let label = labels.get(p.line).filter(|l| p.starts_line && !l.is_empty());
        if let Some(l) = label {
            let glyph = match l.as_str() {
                "•" => "\\textbullet".to_string(),
                "–" => "\\textendash".to_string(),
                "◦" => "\\textopenbullet".to_string(),
                other => escape_text(other),
            };
            let _ = write!(out, "\\llap{{\\makebox[{}bp][l]{{{glyph}}}}}", bp(unit));
        }
        out.push_str(&paragraph_tex(t, &chars, &fmts, pf, p));
        out.push_str("\\par}");
    }
    out
}

/// Plain text of the first Title paragraph on a slide (PDF bookmark).
pub fn slide_title(slide: &Slide) -> Option<String> {
    slide.objects.iter().find_map(|o| match &o.kind {
        ObjectKind::Text(t) => {
            let chars: Vec<char> = t.text.chars().collect();
            rich::paragraphs(&chars).iter().find_map(|p| {
                (t.para(p.line).style == ParaStyle::Title && p.range.start < p.range.end)
                    .then(|| rich::unescape_text(&chars[p.range.clone()]).replace('$', ""))
                    .filter(|s| !s.trim().is_empty())
            })
        }
        _ => None,
    })
}

fn text_node(o: &Object, t: &TextBox) -> String {
    let align = match t.align {
        TextAlign::Left => "left",
        TextAlign::Center => "center",
        TextAlign::Right => "right",
    };
    let mut font = font_cmd(t.font_size);
    if t.bold {
        font.push_str("\\bfseries");
    }
    if t.italic {
        font.push_str("\\itshape");
    }
    let mut opts = format!(
        "anchor=north west,inner sep=0pt,outer sep=0pt,text width={}bp,align={align},font={font},text={}",
        bp(o.rect.w.max(1.0)),
        color_name(t.color)
    );
    if let Some(a) = opacity(t.color) {
        let _ = write!(opts, ",text opacity={a}");
    }
    format!(
        "\\node[{opts}] at ({},{}) {{{}}};\n",
        bp(o.rect.x),
        bp(o.rect.y),
        text_body(t)
    )
}

fn image_node(o: &Object, rel: Option<&String>) -> String {
    match rel {
        Some(rel) => format!(
            "\\node[anchor=north west,inner sep=0pt,outer sep=0pt] at ({},{}) {{\\includegraphics[width={}bp,height={}bp]{{{rel}}}}};\n",
            bp(o.rect.x),
            bp(o.rect.y),
            bp(o.rect.w.max(0.1)),
            bp(o.rect.h.max(0.1))
        ),
        None => format!(
            "\\draw[red,line width=0.5bp] ({x},{y}) rectangle ({r},{b}) ({x},{y}) -- ({r},{b}) ({x},{b}) -- ({r},{y});\n",
            x = bp(o.rect.x),
            y = bp(o.rect.y),
            r = bp(o.rect.right()),
            b = bp(o.rect.bottom())
        ),
    }
}

/// Arrow-head size used by both the editor and the export.
pub fn arrow_head(stroke_width: f32) -> (f32, f32) {
    let len = 3.0 + 3.0 * stroke_width;
    (len, len * 0.8)
}

fn shape_cmd(o: &Object, s: &Shape) -> String {
    let r = &o.rect;
    let mut opts: Vec<String> = Vec::new();
    let has_stroke = s.stroke_width > 0.0;
    if has_stroke {
        opts.push(format!("draw={}", color_name(s.stroke)));
        opts.push(format!("line width={}bp", bp(s.stroke_width)));
        if let Some(a) = opacity(s.stroke) {
            opts.push(format!("draw opacity={a}"));
        }
    }
    match s.kind {
        ShapeKind::Rectangle | ShapeKind::Ellipse => {
            if let Some(f) = s.fill {
                opts.push(format!("fill={}", color_name(f)));
                if let Some(a) = opacity(f) {
                    opts.push(format!("fill opacity={a}"));
                }
            }
            if !has_stroke && s.fill.is_none() {
                return String::new();
            }
            let o = opts.join(",");
            if s.kind == ShapeKind::Rectangle {
                format!(
                    "\\path[{o}] ({},{}) rectangle ({},{});\n",
                    bp(r.x),
                    bp(r.y),
                    bp(r.right()),
                    bp(r.bottom())
                )
            } else {
                let (cx, cy) = r.center();
                format!(
                    "\\path[{o}] ({},{}) ellipse [x radius={}bp,y radius={}bp];\n",
                    bp(cx),
                    bp(cy),
                    bp((r.w * 0.5).max(0.01)),
                    bp((r.h * 0.5).max(0.01))
                )
            }
        }
        ShapeKind::Line | ShapeKind::Arrow => {
            if !has_stroke {
                return String::new();
            }
            opts.push("line cap=butt".into());
            if s.kind == ShapeKind::Arrow {
                let (len, wid) = arrow_head(s.stroke_width);
                opts.push(format!("-{{Triangle[length={}bp,width={}bp]}}", bp(len), bp(wid)));
            }
            let (a, b) = s.endpoints(r);
            format!(
                "\\draw[{}] ({},{}) -- ({},{});\n",
                opts.join(","),
                bp(a.0),
                bp(a.1),
                bp(b.0),
                bp(b.1)
            )
        }
    }
}

fn collect_colors(doc: &Document) -> BTreeSet<(u8, u8, u8)> {
    let mut set = BTreeSet::new();
    let mut add = |c: Color| {
        set.insert((c.r, c.g, c.b));
    };
    for s in &doc.slides {
        add(s.background);
        for o in &s.objects {
            match &o.kind {
                ObjectKind::Text(t) => {
                    add(t.color);
                    for sp in &t.spans {
                        if let Some(c) = sp.fmt.color {
                            add(c);
                        }
                    }
                }
                ObjectKind::Image(_) => {}
                ObjectKind::Shape(sh) => {
                    add(sh.stroke);
                    if let Some(f) = sh.fill {
                        add(f);
                    }
                }
            }
        }
    }
    set
}

fn slide_body(slide: &Slide, assets: &HashMap<AssetId, String>) -> String {
    let mut out = String::new();
    for o in &slide.objects {
        out.push_str(&match &o.kind {
            // Empty placeholders ("Click to add title") are editor-only.
            ObjectKind::Text(t) if t.text.trim().is_empty() => String::new(),
            ObjectKind::Text(t) => text_node(o, t),
            ObjectKind::Image(i) => image_node(o, assets.get(&i.asset)),
            ObjectKind::Shape(s) => shape_cmd(o, s),
        });
    }
    out
}

/// Generate the complete Beamer source. `assets` maps asset ids to paths
/// relative to the `.tex` file.
pub fn generate_tex(doc: &Document, assets: &HashMap<AssetId, String>) -> String {
    let mut t = String::with_capacity(4096 + doc.object_count() * 160);
    let _ = writeln!(
        t,
        "% Generated by BeamerX {} -- https://github.com/moaziat/BeamerX",
        env!("CARGO_PKG_VERSION")
    );
    t.push_str("% Compile with Tectonic (`tectonic main.tex`) or XeLaTeX/LuaLaTeX run twice\n");
    t.push_str("% (`latexmk -xelatex main.tex`). Objects are positioned absolutely with TikZ\n");
    t.push_str("% (remember picture), which needs two LaTeX passes.\n");
    t.push_str("% !TeX program = xelatex\n");
    t.push_str(PRE_CLASS);
    t.push_str("\\documentclass[aspectratio=169,11pt]{beamer}\n");
    t.push_str(MATH_PACKAGES);
    t.push_str("\\usepackage{graphicx}\n\\usepackage{tikz}\n\\usetikzlibrary{arrows.meta}\n");
    t.push_str("\\usefonttheme[onlymath]{serif}\n");
    // Inline math is never split across lines (the editor keeps each
    // formula on one line too).
    t.push_str("\\relpenalty=10000\n\\binoppenalty=10000\n");
    t.push_str("\\setbeamertemplate{navigation symbols}{}\n");
    for (r, g, b) in collect_colors(doc) {
        let _ = writeln!(
            t,
            "\\definecolor{{{}}}{{RGB}}{{{r},{g},{b}}}",
            color_name(Color::rgb(r, g, b))
        );
    }
    let _ = writeln!(t, "\\title{{{}}}", escape_text(&doc.meta.title));
    if !doc.meta.author.trim().is_empty() {
        let _ = writeln!(t, "\\author{{{}}}", escape_text(&doc.meta.author));
    }
    let _ = writeln!(
        t,
        "\\hypersetup{{pdftitle={{{}}},pdfauthor={{{}}}}}",
        escape_text(&doc.meta.title),
        escape_text(&doc.meta.author)
    );
    if !doc.meta.preamble.trim().is_empty() {
        t.push_str("% ---- user preamble ----\n");
        t.push_str(doc.meta.preamble.trim_end());
        t.push_str("\n% ---- end user preamble ----\n");
    }
    t.push_str("\\begin{document}\n");
    for (n, slide) in doc.slides.iter().enumerate() {
        let _ = writeln!(t, "% ---------------- slide {} ----------------", n + 1);
        t.push_str("{\n");
        let _ = writeln!(
            t,
            "\\setbeamercolor{{background canvas}}{{bg={}}}",
            color_name(slide.background)
        );
        t.push_str("\\begin{frame}[plain]\n");
        if let Some(title) = slide_title(slide) {
            // Slide titles become the PDF outline.
            let _ = writeln!(
                t,
                "\\pdfbookmark[1]{{{}}}{{bxslide{}}}",
                escape_text(title.trim()),
                n + 1
            );
        }
        t.push_str("\\begin{tikzpicture}[remember picture,overlay]\n");
        t.push_str("\\begin{scope}[shift={(current page.north west)},x=1bp,y=-1bp]\n");
        // An invisible anchor keeps empty slides valid.
        t.push_str("\\path (0,0);\n");
        t.push_str(&slide_body(slide, assets));
        t.push_str("\\end{scope}\n\\end{tikzpicture}\n\\end{frame}\n}\n");
    }
    t.push_str("\\end{document}\n");
    t
}

/// File name used for an asset inside an exported package: a readable stem
/// from the original name plus a hash prefix for uniqueness.
pub fn export_file_name(meta: &AssetMeta) -> String {
    let stem = Path::new(&meta.original_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut clean: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    clean = clean.trim_matches('-').to_string();
    if clean.is_empty() {
        clean = "image".into();
    }
    format!("{clean}-{}.{}", &meta.id.0[..10], meta.format.ext())
}

/// Write `main.tex` and `images/` into `dir` (which must exist).
pub fn write_package_into(dir: &Path, snap: &SaveSnapshot) -> Result<PathBuf> {
    let img_dir = dir.join("images");
    let mut map = HashMap::new();
    if !snap.assets.is_empty() {
        fs::create_dir_all(&img_dir)?;
    }
    for (meta, src) in &snap.assets {
        let name = export_file_name(meta);
        fs::copy(src, img_dir.join(&name)).with_context(|| format!("cannot copy image {}", meta.original_name))?;
        map.insert(meta.id.clone(), format!("images/{name}"));
    }
    let tex = generate_tex(&snap.doc, &map);
    let main = dir.join(MAIN_TEX);
    fs::write(&main, tex)?;
    Ok(main)
}

fn sibling_temp(target: &Path, tag: &str) -> PathBuf {
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("export");
    target.with_file_name(format!(".{name}.{tag}-{}", std::process::id()))
}

/// Export a self-contained Beamer folder (`main.tex` + `images/`) to
/// `target`. Built in a sibling temporary directory and moved into place, so
/// a failure never leaves a half-written export behind.
pub fn export_tex_package(target: &Path, snap: &SaveSnapshot) -> Result<()> {
    let _span = tracing::info_span!("export_tex", path = %target.display()).entered();
    let tmp = sibling_temp(target, "exporting");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).with_context(|| format!("cannot create {}", tmp.display()))?;
    let res = write_package_into(&tmp, snap).and_then(|_| {
        if target.exists() {
            if !target.is_dir() {
                bail!("{} exists and is not a folder", target.display());
            }
            let old = sibling_temp(target, "replaced");
            let _ = fs::remove_dir_all(&old);
            fs::rename(target, &old)?;
            if let Err(e) = fs::rename(&tmp, target) {
                let _ = fs::rename(&old, target);
                return Err(e.into());
            }
            let _ = fs::remove_dir_all(&old);
        } else {
            fs::rename(&tmp, target)?;
        }
        Ok(())
    });
    if res.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    res
}

/// Export a PDF by compiling the Beamer package in a temporary directory.
/// The temporary directory is always removed; the target is replaced
/// atomically only after a successful compile.
pub fn export_pdf(target: &Path, snap: &SaveSnapshot, tc: &Toolchain, cancel: &AtomicBool) -> Result<()> {
    let _span = tracing::info_span!("export_pdf", path = %target.display()).entered();
    let work = std::env::temp_dir().join(format!(
        "BeamerX/export-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&work)?;
    let res = (|| -> Result<()> {
        write_package_into(&work, snap)?;
        let pdf = tc.compile(&work, MAIN_TEX, &CompileOptions::document(), cancel)?;
        let tmp_target = sibling_temp(target, "pdf");
        fs::copy(&pdf, &tmp_target).with_context(|| format!("cannot write {}", target.display()))?;
        fs::rename(&tmp_target, target).inspect_err(|_| {
            let _ = fs::remove_file(&tmp_target);
        })?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::*;

    #[test]
    fn escapes_all_specials() {
        let s = escape_text(r"a\b{c}$d&e#f%g_h^i~j<k>l|m");
        assert_eq!(
            s,
            r"a\textbackslash{}b\{c\}\$d\&e\#f\%g\_h\textasciicircum{}i\textasciitilde{}j\textless{}k\textgreater{}l\textbar{}m"
        );
        assert_eq!(escape_text("x\u{7}y\tz"), "xy z");
        assert_eq!(escape_text("Größe – π"), "Größe – π");
    }

    #[test]
    fn math_normalization_and_checks() {
        assert_eq!(normalize_math("a\n\n\nb % c"), "a\nb % c\n");
        assert_eq!(math_problem(r"\frac{a}{b}"), None);
        assert_eq!(math_problem(r"\{ x \}"), None);
        assert_eq!(math_problem(r"x % }"), None);
        assert!(math_problem(r"\frac{a}{b").is_some());
        assert!(math_problem("a}{").is_some());
        assert!(math_problem("a $ b").is_some());
        assert!(math_problem(r"x \end{document}").is_some());
    }

    #[test]
    fn rich_text_exports_styles_lists_and_spans() {
        let mut t = TextBox {
            text: "Title\nbold red\nsub".into(),
            font_size: 10.0,
            ..Default::default()
        };
        t.format_lines(0, 0, |p| p.style = ParaStyle::Title);
        t.format_lines(1, 2, |p| p.list = ListKind::Number);
        t.format_lines(2, 2, |p| p.level = 1);
        t.format_range(6, 10, |f| f.bold = Some(true));
        t.format_range(11, 14, |f| f.color = Some(Color::rgb(255, 0, 0)));
        let body = text_body(&t);
        assert!(
            body.contains("{\\fontsize{18bp}{21.6bp}\\selectfont\\bfseries\\strut Title\\par}"),
            "{body}"
        );
        assert!(
            body.contains("\\advance\\leftskip by 10bp\\relax\\strut \\llap{\\makebox[10bp][l]{1.}}{\\bfseries{}bold} {\\color{bxff0000}{}red}\\par}"),
            "{body}"
        );
        assert!(
            body.contains("\\advance\\leftskip by 20bp\\relax\\strut \\llap{\\makebox[10bp][l]{1.}}sub"),
            "{body}"
        );
        let mut slide = Slide::new(SlideId(1));
        slide.objects.push(Object {
            id: ObjectId(2),
            rect: Rect::new(0.0, 0.0, 100.0, 20.0),
            kind: ObjectKind::Text(t),
        });
        assert_eq!(slide_title(&slide).as_deref(), Some("Title"));
    }

    #[test]
    fn number_formatting() {
        assert_eq!(bp(1.0), "1");
        assert_eq!(bp(1.25), "1.25");
        assert_eq!(bp(-0.00001), "0");
        assert_eq!(bp(453.5433), "453.5433");
    }

    #[test]
    fn generated_tex_structure() {
        let mut doc = Document::new();
        doc.meta.title = "T & A".into();
        let sid = doc.slides[0].id;
        let ids: Vec<ObjectId> = (0..3).map(|_| doc.alloc_object_id()).collect();
        let s = doc.slide_mut(sid).unwrap();
        s.objects.push(Object {
            id: ids[0],
            rect: Rect::new(10.0, 20.0, 100.0, 30.0),
            kind: ObjectKind::Text(TextBox {
                text: "50% off\n\n_x_".into(),
                ..Default::default()
            }),
        });
        s.objects.push(Object {
            id: ids[1],
            rect: Rect::new(10.0, 60.0, 10.0, 10.0),
            kind: ObjectKind::Text(TextBox {
                text: r"Energy $\alpha^2 % note$ costs \$5 $$\int f$$ end".into(),
                ..Default::default()
            }),
        });
        s.objects.push(Object {
            id: ids[2],
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            kind: ObjectKind::Image(ImageRef {
                asset: AssetId("b".repeat(64)),
            }),
        });
        let tex = generate_tex(&doc, &HashMap::new());
        assert!(tex.starts_with("% Generated by BeamerX"));
        assert!(tex.contains("\\documentclass[aspectratio=169,11pt]{beamer}"));
        assert!(tex.contains("\\title{T \\& A}"));
        // Each line is its own paragraph with its size and a strut.
        let f = "\\fontsize{20bp}{24bp}\\selectfont";
        assert!(tex.contains(&format!(
            "at (10,20) {{\\parindent=0pt\\parskip=0pt\\relax {{{f}\\strut 50\\% off\\par}}{{{f}\\strut \\par}}{{{f}\\strut \\_x\\_\\par}}}}"
        )));
        // Inline math passes through; text dollars stay escaped; display
        // math is centred in its own paragraph.
        assert!(tex.contains(&format!(
            "{{{f}\\strut Energy $\\alpha^2 % note\n$ costs \\$5 \\par}}{{{f}\\strut \\hspace*{{\\fill}}$\\displaystyle \\int f\n$\\hspace*{{\\fill}}\\par}}{{{f}\\strut  end\\par}}"
        )));
        // Missing image becomes a visible placeholder, not a broken \includegraphics.
        assert!(!tex.contains("includegraphics"));
        assert_eq!(tex.matches("\\begin{frame}").count(), 1);
        assert!(tex.trim_end().ends_with("\\end{document}"));
    }
}
