//! Discovery and invocation of an external LaTeX toolchain.
//!
//! BeamerX does not bundle a TeX distribution. It uses a separately
//! installed engine: Tectonic (preferred, self-contained, fetches packages on
//! demand) or a TeX Live/MacTeX XeLaTeX/LuaLaTeX/pdfLaTeX. Apps launched from
//! Finder do not inherit the shell `PATH`, so common install locations are
//! searched explicitly.

use anyhow::{Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    Tectonic,
    XeLaTeX,
    LuaLaTeX,
    PdfLaTeX,
}

#[derive(Clone, Debug)]
pub struct Toolchain {
    pub kind: EngineKind,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct CompileOptions {
    /// Run exactly one pass (enough for isolated equations).
    pub single_pass: bool,
    pub timeout: Duration,
}

impl CompileOptions {
    pub fn document() -> Self {
        Self {
            single_pass: false,
            timeout: Duration::from_secs(600),
        }
    }
    pub fn preview() -> Self {
        Self {
            single_pass: true,
            timeout: Duration::from_secs(120),
        }
    }
}

/// A compile failure with the relevant part of the TeX log.
#[derive(Debug)]
pub struct CompileError {
    pub summary: String,
    pub log_excerpt: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.summary)?;
        if !self.log_excerpt.is_empty() {
            write!(f, "\n{}", self.log_excerpt)?;
        }
        Ok(())
    }
}
impl std::error::Error for CompileError {}

#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}
impl std::error::Error for Cancelled {}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/Library/TeX/texbin",
        "/opt/local/bin",
        "/usr/bin",
    ] {
        dirs.push(PathBuf::from(d));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join("bin"));
        dirs.push(home.join(".local/bin"));
    }
    dirs
}

fn find_exe(name: &str) -> Option<PathBuf> {
    search_dirs().into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

impl Toolchain {
    /// Find an engine. `BEAMERX_LATEX` may point at a specific executable.
    pub fn detect() -> Option<Toolchain> {
        if let Some(p) = std::env::var_os("BEAMERX_LATEX").map(PathBuf::from)
            && p.is_file()
        {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let kind = if name.contains("tectonic") {
                EngineKind::Tectonic
            } else if name.contains("lualatex") {
                EngineKind::LuaLaTeX
            } else if name.contains("pdflatex") {
                EngineKind::PdfLaTeX
            } else {
                EngineKind::XeLaTeX
            };
            return Some(Toolchain { kind, path: p });
        }
        for (name, kind) in [
            ("tectonic", EngineKind::Tectonic),
            ("xelatex", EngineKind::XeLaTeX),
            ("lualatex", EngineKind::LuaLaTeX),
            ("pdflatex", EngineKind::PdfLaTeX),
        ] {
            if let Some(path) = find_exe(name) {
                return Some(Toolchain { kind, path });
            }
        }
        None
    }

    pub fn describe(&self) -> String {
        let name = match self.kind {
            EngineKind::Tectonic => "Tectonic",
            EngineKind::XeLaTeX => "XeLaTeX",
            EngineKind::LuaLaTeX => "LuaLaTeX",
            EngineKind::PdfLaTeX => "pdfLaTeX",
        };
        format!("{name} ({})", self.path.display())
    }

    /// Compile `dir/tex_name` and return the path of the produced PDF.
    /// Shell escape is always disabled. The job can be cancelled through
    /// `cancel`; the child process is then killed.
    pub fn compile(&self, dir: &Path, tex_name: &str, opts: &CompileOptions, cancel: &AtomicBool) -> Result<PathBuf> {
        let stem = Path::new(tex_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();
        let log_path = dir.join(format!("{stem}.beamerx-log"));
        let passes = match (self.kind, opts.single_pass) {
            (EngineKind::Tectonic, _) | (_, true) => 1,
            _ => 2,
        };
        for _ in 0..passes {
            let mut cmd = Command::new(&self.path);
            cmd.current_dir(dir);
            match self.kind {
                EngineKind::Tectonic => {
                    cmd.args(["-X", "compile", "--untrusted", "--keep-logs"]);
                    if opts.single_pass {
                        cmd.args(["-r", "0"]);
                    }
                    cmd.arg(tex_name);
                }
                _ => {
                    cmd.args([
                        "-interaction=nonstopmode",
                        "-halt-on-error",
                        "-no-shell-escape",
                        "-file-line-error",
                    ]);
                    cmd.arg(tex_name);
                }
            }
            let log = fs::File::create(&log_path)?;
            cmd.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
            let started = Instant::now();
            let mut child = cmd
                .spawn()
                .map_err(|e| anyhow::anyhow!("cannot run {}: {e}", self.path.display()))?;
            let status = loop {
                if let Some(st) = child.try_wait()? {
                    break st;
                }
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Cancelled.into());
                }
                if started.elapsed() > opts.timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("LaTeX timed out after {} s", opts.timeout.as_secs());
                }
                std::thread::sleep(Duration::from_millis(15));
            };
            if !status.success() {
                let out = fs::read_to_string(&log_path).unwrap_or_default();
                let tex_log = fs::read_to_string(dir.join(format!("{stem}.log"))).unwrap_or_default();
                return Err(CompileError {
                    summary: "LaTeX reported an error".into(),
                    log_excerpt: extract_errors(&out, &tex_log),
                }
                .into());
            }
        }
        let pdf = dir.join(format!("{stem}.pdf"));
        if !pdf.is_file() {
            bail!("LaTeX finished but produced no PDF");
        }
        Ok(pdf)
    }
}

/// Pull the useful error lines out of engine output and the `.log` file.
pub fn extract_errors(output: &str, tex_log: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for l in output.lines() {
        let t = l.trim();
        if t.starts_with("error:") && !t.contains("halted on potentially-recoverable error") {
            lines.push(t.trim_start_matches("error:").trim().to_string());
        }
    }
    // `-file-line-error` style: "./main.tex:7: Undefined control sequence."
    let is_file_line_error = |l: &str| {
        let mut parts = l.splitn(3, ':');
        matches!((parts.next(), parts.next(), parts.next()),
            (Some(f), Some(n), Some(_)) if f.ends_with(".tex") && n.parse::<u32>().is_ok())
    };
    let log_lines: Vec<&str> = tex_log.lines().collect();
    for (i, l) in log_lines.iter().enumerate() {
        if l.starts_with('!') || is_file_line_error(l) {
            let mut s = l.trim_start_matches('!').trim().to_string();
            if let Some(next) = log_lines.get(i + 1).filter(|n| n.starts_with("l.")) {
                s.push_str(" — ");
                s.push_str(next.trim());
            }
            if !lines.contains(&s) {
                lines.push(s);
            }
        }
    }
    lines.truncate(8);
    if lines.is_empty() {
        output
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tectonic_errors() {
        let out = "note: Running TeX ...\nerror: eq.tex:7: Undefined control sequence\nerror: halted on potentially-recoverable error as specified\n";
        assert_eq!(extract_errors(out, ""), "eq.tex:7: Undefined control sequence");
    }

    #[test]
    fn extracts_texlive_errors() {
        let log = "This is XeTeX\n! Undefined control sequence.\nl.7 $\\foo\n                $\n";
        assert_eq!(extract_errors("", log), "Undefined control sequence. — l.7 $\\foo");
    }
}
