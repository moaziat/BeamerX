//! Text with LaTeX math, written like LaTeX: `$…$` for inline math and
//! `$$…$$` for display math. Everything else is plain text; `\$` is a
//! literal dollar sign.
//!
//! The same structure drives the editor layout and the Beamer exporter, so
//! both agree on where math is and where lines/paragraphs break.
//!
//! All indices are **char** indices into the source string.

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunKind {
    /// Plain text (may contain `\$` escapes).
    Text,
    /// Inline math; `src` is the LaTeX between the dollars.
    Math { src: Range<usize> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub kind: RunKind,
    /// Source range including delimiters.
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParaKind {
    /// A line of text and inline math.
    Normal(Vec<Run>),
    /// A `$$…$$` block shown centred on its own line.
    Display { src: Range<usize> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paragraph {
    pub kind: ParaKind,
    /// Source range of the paragraph (without the separating newline).
    pub range: Range<usize>,
    /// Starts a new logical line (after a newline or at the start), as
    /// opposed to continuing after a display block. Bullets are drawn only
    /// for these.
    pub starts_line: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    Text(Range<usize>),
    Inline { range: Range<usize>, src: Range<usize> },
    Display { range: Range<usize>, src: Range<usize> },
}

/// Find the matching closing delimiter. Inline math may not span lines.
fn find_close(chars: &[char], from: usize, display: bool) -> Option<usize> {
    let n = chars.len();
    let mut j = from;
    while j < n {
        match chars[j] {
            '\\' => j += 2,
            '\n' if !display => return None,
            '$' => {
                if display {
                    return (j + 1 < n && chars[j + 1] == '$').then_some(j);
                }
                return Some(j);
            }
            _ => j += 1,
        }
    }
    None
}

fn segments(chars: &[char]) -> Vec<Seg> {
    let n = chars.len();
    let mut out = Vec::new();
    let mut text_start = 0;
    let mut i = 0;
    while i < n {
        match chars[i] {
            '\\' => i += 2,
            '$' => {
                let display = i + 1 < n && chars[i + 1] == '$';
                let open = if display { 2 } else { 1 };
                let close = find_close(chars, i + open, display);
                match close {
                    Some(j) if chars[i + open..j].iter().any(|c| !c.is_whitespace()) => {
                        if text_start < i {
                            out.push(Seg::Text(text_start..i));
                        }
                        let range = i..j + open;
                        let src = i + open..j;
                        out.push(if display {
                            Seg::Display { range, src }
                        } else {
                            Seg::Inline { range, src }
                        });
                        i = j + open;
                        text_start = i;
                    }
                    // Unclosed (or empty) math is literal text.
                    _ => i += open,
                }
            }
            _ => i += 1,
        }
    }
    if text_start < n {
        out.push(Seg::Text(text_start..n.max(text_start)));
    }
    out
}

/// Split source text into paragraphs: newlines (outside math) end a
/// paragraph, and every display block is a paragraph of its own.
pub fn paragraphs(chars: &[char]) -> Vec<Paragraph> {
    let mut paras = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    let mut start = 0usize;
    let mut starts_line = true;
    // Just after a display block: a newline there only ends the block's line.
    let mut after_display = false;
    let flush = |paras: &mut Vec<Paragraph>, runs: &mut Vec<Run>, start: usize, end: usize, starts_line: bool| {
        paras.push(Paragraph {
            kind: ParaKind::Normal(std::mem::take(runs)),
            range: start..end,
            starts_line,
        });
    };
    for seg in segments(chars) {
        match seg {
            Seg::Text(r) => {
                let mut s = r.start;
                for k in r.clone() {
                    if chars[k] == '\n' {
                        if s < k {
                            runs.push(Run {
                                kind: RunKind::Text,
                                range: s..k,
                            });
                        }
                        if after_display && runs.is_empty() && start == k {
                            after_display = false;
                            start = k + 1;
                            starts_line = true;
                            s = k + 1;
                            continue;
                        }
                        after_display = false;
                        flush(&mut paras, &mut runs, start, k, starts_line);
                        start = k + 1;
                        starts_line = true;
                        s = k + 1;
                    }
                }
                if s < r.end {
                    runs.push(Run {
                        kind: RunKind::Text,
                        range: s..r.end,
                    });
                    after_display = false;
                }
            }
            Seg::Inline { range, src } => {
                after_display = false;
                runs.push(Run {
                    kind: RunKind::Math { src },
                    range,
                })
            }
            Seg::Display { range, src } => {
                // Text before the block on the same line becomes its own paragraph.
                if !runs.is_empty() {
                    flush(&mut paras, &mut runs, start, range.start, starts_line);
                    starts_line = false;
                }
                paras.push(Paragraph {
                    kind: ParaKind::Display { src },
                    range: range.clone(),
                    starts_line,
                });
                start = range.end;
                starts_line = false;
                after_display = true;
            }
        }
    }
    // Trailing paragraph (also an empty one after a final newline, or the
    // only paragraph of an empty text). Skip an empty tail after a display.
    let n = chars.len();
    if !runs.is_empty() || (starts_line && !after_display) || paras.is_empty() {
        flush(&mut paras, &mut runs, start, n, starts_line);
    }
    paras
}

/// Plain text of a text run with `\$` unescaped.
pub fn unescape_text(chars: &[char]) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1) == Some(&'$') {
            out.push('$');
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

pub fn slice(chars: &[char], r: &Range<usize>) -> String {
    chars[r.start.min(chars.len())..r.end.min(chars.len())].iter().collect()
}

/// Does the text contain any (closed) math?
pub fn has_math(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    segments(&chars).iter().any(|s| !matches!(s, Seg::Text(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(s: &str) -> (Vec<char>, Vec<Paragraph>) {
        let c: Vec<char> = s.chars().collect();
        let p = paragraphs(&c);
        (c, p)
    }

    fn describe(s: &str) -> Vec<String> {
        let (c, ps) = paras(s);
        ps.iter()
            .map(|p| match &p.kind {
                ParaKind::Display { src } => format!("D[{}]", slice(&c, src)),
                ParaKind::Normal(runs) => runs
                    .iter()
                    .map(|r| match &r.kind {
                        RunKind::Text => format!("T[{}]", slice(&c, &r.range)),
                        RunKind::Math { src } => format!("M[{}]", slice(&c, src)),
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            })
            .collect()
    }

    #[test]
    fn inline_math_is_recognized_only_when_closed() {
        assert_eq!(describe("value $f(x)$ here"), ["T[value ]M[f(x)]T[ here]"]);
        assert_eq!(describe("value $f(x"), ["T[value $f(x]"]);
        assert_eq!(describe("cost \\$5 and $x$"), ["T[cost \\$5 and ]M[x]"]);
        assert_eq!(describe("$ $ empty"), ["T[$ $ empty]"]);
        assert_eq!(describe("$a$$b$"), ["M[a]M[b]"]);
    }

    #[test]
    fn inline_math_does_not_span_lines() {
        assert_eq!(describe("a $x\ny$ b"), ["T[a $x]", "T[y$ b]"]);
    }

    #[test]
    fn display_math_gets_its_own_paragraph() {
        assert_eq!(
            describe("intro $$\\int f$$ outro"),
            ["T[intro ]", "D[\\int f]", "T[ outro]"]
        );
        let (_, ps) = paras("intro $$x$$ outro");
        assert_eq!(
            ps.iter().map(|p| p.starts_line).collect::<Vec<_>>(),
            [true, false, false]
        );
        assert_eq!(describe("$$a$$"), ["D[a]"]);
        assert_eq!(describe("$$a$$\nnext"), ["D[a]", "T[next]"]);
        assert_eq!(describe("$$a$ b"), ["T[$$a$ b]"]);
    }

    #[test]
    fn newlines_make_paragraphs_with_ranges() {
        let (_, ps) = paras("ab\n\ncd\n");
        assert_eq!(ps.len(), 4);
        assert_eq!(ps[0].range, 0..2);
        assert_eq!(ps[1].range, 3..3);
        assert_eq!(ps[2].range, 4..6);
        assert_eq!(ps[3].range, 7..7);
        assert_eq!(paras("").1.len(), 1);
    }

    #[test]
    fn unescape_and_detection() {
        assert_eq!(unescape_text(&"a\\$b\\x".chars().collect::<Vec<_>>()), "a$b\\x");
        assert!(has_math("x $y$"));
        assert!(!has_math("costs $5"));
    }
}
