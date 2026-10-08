//! Progress bars on stderr (PLAN §5.7): one overall bar plus one per file in progress in
//! directory mode. Hidden when stderr is not a terminal and with `--quiet`. Everything the run
//! prints while bars are up goes through `line`, so the bars do not tear.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

/// Set while bars are on screen.
static BARS: OnceLock<MultiProgress> = OnceLock::new();

/// Prints one line on stderr without tearing the bars.
pub fn line(msg: impl AsRef<str>) {
    match BARS.get() {
        Some(mp) if mp.println(msg.as_ref()).is_ok() => {}
        _ => eprintln!("{}", msg.as_ref()),
    }
}

pub struct Progress {
    overall: ProgressBar,
    files: Mutex<HashMap<usize, ProgressBar>>,
    multi: Option<MultiProgress>,
    per_file: bool,
}

impl Progress {
    /// `per_file`: also draw a bar per file in progress (directory mode).
    pub fn new(quiet: bool, per_file: bool) -> Self {
        let target = ProgressDrawTarget::stderr_with_hz(10);
        // Also hidden for TERM=dumb, where `println` would swallow the lines.
        if quiet || !std::io::stderr().is_terminal() || target.is_hidden() {
            return Self {
                overall: ProgressBar::hidden(),
                files: Mutex::default(),
                multi: None,
                per_file: false,
            };
        }
        let multi = MultiProgress::with_draw_target(target);
        let overall = multi.add(ProgressBar::new(0));
        overall.set_style(
            ProgressStyle::with_template(
                "{prefix:>10} [{bar:30.cyan/blue}] {pos}/{len} pages  {per_sec}  eta {eta}",
            )
            .unwrap()
            .progress_chars("=> "),
        );
        overall.set_prefix("total");
        overall.enable_steady_tick(Duration::from_millis(200));
        let _ = BARS.set(multi.clone());
        Self {
            overall,
            files: Mutex::default(),
            multi: Some(multi),
            per_file,
        }
    }

    pub fn add_total(&self, pages: usize) {
        self.overall.inc_length(pages as u64);
    }

    pub fn inc(&self, pages: usize) {
        self.overall.inc(pages as u64);
    }

    pub fn file_start(&self, id: usize, name: &str, total: usize, done: usize) {
        let Some(multi) = self.multi.as_ref().filter(|_| self.per_file) else {
            return;
        };
        let bar = multi.add(ProgressBar::new(total as u64));
        bar.set_style(
            ProgressStyle::with_template("{prefix:>10.dim} [{bar:30.green/white}] {pos}/{len}")
                .unwrap()
                .progress_chars("=> "),
        );
        bar.set_prefix(name.chars().take(10).collect::<String>());
        bar.set_position(done as u64);
        self.files.lock().unwrap().insert(id, bar);
    }

    pub fn file_inc(&self, id: usize) {
        if let Some(bar) = self.files.lock().unwrap().get(&id) {
            bar.inc(1);
        }
    }

    pub fn file_done(&self, id: usize) {
        if let Some(bar) = self.files.lock().unwrap().remove(&id) {
            bar.finish_and_clear();
        }
    }

    pub fn finish(&self) {
        self.overall.finish_and_clear();
    }
}
