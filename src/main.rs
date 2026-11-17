use beamerx::app::BeamerApp;
use beamerx::bench::{Bench, Scenario};
use std::path::PathBuf;
use std::time::Instant;

const USAGE: &str = "\
BeamerX — lightweight native editor for scientific Beamer presentations

USAGE:
    beamerx [FILE.beamerx]
    beamerx --bench <idle|drag|switch|images|saveload> [--out FILE.json] [--seconds N]
    beamerx --version | --help

ENVIRONMENT:
    BEAMERX_LATEX   path to a LaTeX engine (tectonic, xelatex, lualatex, pdflatex)
    BEAMERX_LOG     log filter, e.g. `beamerx=debug` (default: warn,beamerx=info)
";

fn main() -> eframe::Result {
    let process_start = Instant::now();
    let filter = std::env::var("BEAMERX_LOG").unwrap_or_else(|_| "warn,beamerx=info".into());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_target(false)
        .init();

    let mut args = std::env::args().skip(1);
    let mut open: Option<PathBuf> = None;
    let mut bench: Option<Scenario> = None;
    let mut out: Option<PathBuf> = None;
    let mut seconds = 35u64;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            "--version" | "-V" => {
                println!("beamerx {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--bench" => match args.next().map(|s| s.parse::<Scenario>()) {
                Some(Ok(s)) => bench = Some(s),
                Some(Err(e)) => {
                    eprintln!("{e}");
                    std::process::exit(2);
                }
                None => {
                    eprintln!("--bench needs a scenario");
                    std::process::exit(2);
                }
            },
            "--out" => out = args.next().map(PathBuf::from),
            "--seconds" => seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(seconds),
            // macOS passes -psn_* when launched from Finder on old systems.
            s if s.starts_with("-psn_") => {}
            s if s.starts_with('-') => {
                eprintln!("unknown option {s}\n\n{USAGE}");
                std::process::exit(2);
            }
            s => open = Some(PathBuf::from(s)),
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("BeamerX")
            .with_app_id("io.github.moaziat.beamerx")
            .with_inner_size([1360.0, 840.0])
            .with_min_inner_size([860.0, 520.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let bench = bench.map(|s| Bench::new(s, process_start, out, seconds));
    eframe::run_native(
        "BeamerX",
        options,
        Box::new(move |cc| Ok(Box::new(BeamerApp::new(cc, open, bench)))),
    )
}
