//! Slide layouts (PowerPoint's "New Slide" gallery): title and content
//! placeholders that show a hint until text is typed. Empty placeholders are
//! not exported.

use crate::document::{
    Document, ListKind, Object, ObjectKind, ParaFmt, ParaStyle, Rect, SLIDE_H, SLIDE_W, Slide, TextAlign, TextBox,
    typeset,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    TitleSlide,
    TitleAndContent,
    SectionHeader,
    TwoContent,
    TitleOnly,
    Blank,
}

impl Layout {
    pub const ALL: [Layout; 6] = [
        Layout::TitleSlide,
        Layout::TitleAndContent,
        Layout::SectionHeader,
        Layout::TwoContent,
        Layout::TitleOnly,
        Layout::Blank,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Layout::TitleSlide => "Title Slide",
            Layout::TitleAndContent => "Title and Content",
            Layout::SectionHeader => "Section Header",
            Layout::TwoContent => "Two Content",
            Layout::TitleOnly => "Title Only",
            Layout::Blank => "Blank",
        }
    }
}

const MARGIN: f32 = 28.0;

fn placeholder(hint: &str, size: f32, style: ParaStyle, list: ListKind, align: TextAlign) -> TextBox {
    TextBox {
        font_size: size,
        align,
        paras: vec![ParaFmt { style, list, level: 0 }]
            .into_iter()
            .filter(|p| !p.is_default())
            .collect(),
        placeholder: Some(hint.into()),
        ..Default::default()
    }
}

fn boxed(doc: &mut Document, t: TextBox, x: f32, y: f32, w: f32) -> Object {
    let h = t.max_size() * typeset::LINE_SPREAD;
    Object {
        id: doc.alloc_object_id(),
        rect: Rect::new(x, y, w, h),
        kind: ObjectKind::Text(t),
    }
}

/// Placeholder objects for `layout` (ids allocated from `doc`).
pub fn objects(doc: &mut Document, layout: Layout) -> Vec<Object> {
    let full = SLIDE_W - 2.0 * MARGIN;
    let title = |hint: &str, align| placeholder(hint, 16.0, ParaStyle::Title, ListKind::None, align);
    let body = |hint: &str| placeholder(hint, 16.0, ParaStyle::Body, ListKind::Bullet, TextAlign::Left);
    match layout {
        Layout::TitleSlide => vec![
            boxed(
                doc,
                placeholder(
                    "Click to add title",
                    20.0,
                    ParaStyle::Title,
                    ListKind::None,
                    TextAlign::Center,
                ),
                MARGIN,
                SLIDE_H * 0.30,
                full,
            ),
            boxed(
                doc,
                placeholder(
                    "Click to add subtitle",
                    14.0,
                    ParaStyle::Subtitle,
                    ListKind::None,
                    TextAlign::Center,
                ),
                MARGIN,
                SLIDE_H * 0.58,
                full,
            ),
        ],
        Layout::TitleAndContent => vec![
            boxed(doc, title("Click to add title", TextAlign::Left), MARGIN, 16.0, full),
            boxed(doc, body("Click to add text"), MARGIN, 64.0, full),
        ],
        Layout::SectionHeader => vec![
            boxed(
                doc,
                title("Click to add section title", TextAlign::Left),
                MARGIN,
                SLIDE_H * 0.40,
                full,
            ),
            boxed(
                doc,
                placeholder(
                    "Click to add text",
                    14.0,
                    ParaStyle::Subtitle,
                    ListKind::None,
                    TextAlign::Left,
                ),
                MARGIN,
                SLIDE_H * 0.62,
                full,
            ),
        ],
        Layout::TwoContent => {
            let half = (full - 16.0) * 0.5;
            vec![
                boxed(doc, title("Click to add title", TextAlign::Left), MARGIN, 16.0, full),
                boxed(doc, body("Click to add text"), MARGIN, 64.0, half),
                boxed(doc, body("Click to add text"), MARGIN + half + 16.0, 64.0, half),
            ]
        }
        Layout::TitleOnly => vec![boxed(
            doc,
            title("Click to add title", TextAlign::Left),
            MARGIN,
            16.0,
            full,
        )],
        Layout::Blank => Vec::new(),
    }
}

/// A new presentation: one Title Slide, like PowerPoint.
pub fn new_presentation() -> Document {
    let mut doc = Document::new();
    let objs = objects(&mut doc, Layout::TitleSlide);
    doc.slides[0].objects = objs;
    doc
}

/// A slide with `layout`'s placeholders.
pub fn new_slide(doc: &mut Document, layout: Layout) -> Slide {
    let id = doc.alloc_slide_id();
    let objects = objects(doc, layout);
    Slide {
        id,
        objects,
        background: crate::document::Color::WHITE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_create_placeholders_inside_the_slide() {
        let mut doc = Document::new();
        for l in Layout::ALL {
            let s = new_slide(&mut doc, l);
            for o in &s.objects {
                assert!(
                    o.rect.x >= 0.0 && o.rect.right() <= SLIDE_W && o.rect.bottom() <= SLIDE_H,
                    "{l:?} {:?}",
                    o.rect
                );
                match &o.kind {
                    ObjectKind::Text(t) => assert!(t.placeholder.is_some() && t.text.is_empty()),
                    _ => panic!("only text placeholders"),
                }
            }
        }
        let d = new_presentation();
        assert_eq!(d.slides[0].objects.len(), 2);
        let mut d2 = d.clone();
        d2.validate_and_repair().unwrap();
        assert_eq!(d, d2);
    }
}
