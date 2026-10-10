//! Opt-in timing telemetry (`--timing-json`, and the per-request `--verbose` lines): what each
//! model request cost, as the client saw it and as the server reports it. Nothing here touches
//! the OCR output or the checkpoints, and nothing is collected unless `enable` was called.
//!
//! Requests are collected in a thread-local list: a consumer thread handles one page at a time,
//! so `take` after the page's `process` returns exactly that page's requests.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// `usage` of an OpenAI-style reply; every field optional.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
}

/// `timings` of an mlx-vlm or llama.cpp reply (llama.cpp field names); every field optional.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Timings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_n: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_n: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicted_n: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicted_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_memory: Option<f64>,
}

/// What the server said about one request.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ServerMetrics {
    pub usage: Usage,
    pub timings: Timings,
}

fn uint(v: &Value, key: &str) -> Option<u64> {
    let x = &v[key];
    x.as_u64().or_else(|| x.as_f64().map(|f| f.max(0.0) as u64))
}

fn float(v: &Value, key: &str) -> Option<f64> {
    v[key].as_f64()
}

/// Reads `usage` and `timings` from a reply, leniently: anything missing or of another type is
/// left out.
pub fn parse_metrics(reply: &Value) -> ServerMetrics {
    let u = &reply["usage"];
    let t = &reply["timings"];
    ServerMetrics {
        usage: Usage {
            prompt_tokens: uint(u, "prompt_tokens"),
            completion_tokens: uint(u, "completion_tokens"),
            cached_tokens: uint(&u["prompt_tokens_details"], "cached_tokens"),
        },
        timings: Timings {
            prompt_n: uint(t, "prompt_n"),
            cache_n: uint(t, "cache_n"),
            predicted_n: uint(t, "predicted_n"),
            prompt_ms: float(t, "prompt_ms"),
            predicted_ms: float(t, "predicted_ms"),
            peak_memory: float(t, "peak_memory"),
        },
    }
}

/// One attempt at a model request.
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub label: String,
    pub image_bytes: usize,
    pub wall_s: f64,
    /// first, empty_retry, length_retry, loop_retry, parse_retry, transport_retry, after_restart.
    pub reason: &'static str,
    /// `ok`, or what went wrong (timeout, connection, HTTP status, bad reply).
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repetition_penalty: Option<f64>,
    #[serde(flatten)]
    pub usage: Usage,
    #[serde(flatten)]
    pub timings: Timings,
}

impl Request {
    fn prompt_tokens(&self) -> Option<u64> {
        self.usage.prompt_tokens.or(self.timings.prompt_n)
    }

    fn completion_tokens(&self) -> Option<u64> {
        self.usage.completion_tokens.or(self.timings.predicted_n)
    }

    /// `812 KB, 3.4s (prompt 1260 tok 0.9s, decode 410 tok 2.3s)` for `--verbose`.
    pub fn describe(&self) -> String {
        let mut s = format!("{} KB, {:.1}s", self.image_bytes / 1024, self.wall_s);
        let part = |name: &str, tok: Option<u64>, ms: Option<f64>| {
            let mut p = String::from(name);
            if let Some(t) = tok {
                p += &format!(" {t} tok");
            }
            if let Some(ms) = ms {
                p += &format!(" {:.1}s", ms / 1000.0);
            }
            (p.len() > name.len()).then_some(p)
        };
        let parts: Vec<String> = [
            part("prompt", self.prompt_tokens(), self.timings.prompt_ms),
            part(
                "decode",
                self.completion_tokens(),
                self.timings.predicted_ms,
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !parts.is_empty() {
            s += &format!(" ({})", parts.join(", "));
        }
        if self.reason != "first" {
            s += &format!(" [{}]", self.reason);
        }
        if self.outcome != "ok" {
            s += &format!(" [{}]", self.outcome);
        }
        s
    }
}

#[derive(Default)]
struct Collector {
    requests: Vec<Request>,
    /// (image key, the last reply had no text) of the previous successful attempt.
    last: Option<(u64, bool)>,
}

thread_local! {
    static COLLECTOR: RefCell<Collector> = RefCell::new(Collector::default());
}

/// Identifies an image without hashing all of it.
pub fn image_key(url: &str) -> u64 {
    let b = url.as_bytes();
    let mid = b.len() / 2;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.len().hash(&mut h);
    b[mid.saturating_sub(128)..(mid + 128).min(b.len())].hash(&mut h);
    h.finish()
}

/// What an attempt is, from the request itself and the previous attempt on this thread.
pub fn reason(key: u64, max_tokens: u32, base_max: u32, penalty: Option<f64>) -> &'static str {
    COLLECTOR.with(|c| {
        let same = c.borrow().last.filter(|(k, _)| *k == key);
        match same {
            None => "first",
            Some(_) if penalty.is_some() => "loop_retry",
            Some(_) if max_tokens > base_max => "length_retry",
            Some((_, true)) => "empty_retry",
            Some(_) => "parse_retry",
        }
    })
}

/// Records an attempt. `empty`: the reply had no text (None: no reply).
pub fn record(key: u64, req: Request, empty: Option<bool>) {
    COLLECTOR.with(|c| {
        let mut c = c.borrow_mut();
        c.last = empty.map(|e| (key, e)).or(c.last);
        c.requests.push(req);
    });
}

/// The requests recorded on this thread since the last `take`.
pub fn take() -> Vec<Request> {
    COLLECTOR.with(|c| {
        let mut c = c.borrow_mut();
        c.last = None;
        std::mem::take(&mut c.requests)
    })
}

fn secs(d: Duration) -> f64 {
    (d.as_secs_f64() * 1000.0).round() / 1000.0
}

/// One page's timing, as the producer and the consumer measured it.
#[derive(Debug, Default, Clone, Serialize)]
pub struct PageTiming {
    pub page: usize,
    pub prep_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recognize_s: Option<f64>,
    pub requests: Vec<Request>,
}

impl PageTiming {
    pub fn new(
        page: usize,
        prep: Duration,
        recognize: Option<Duration>,
        reqs: Vec<Request>,
    ) -> Self {
        Self {
            page,
            prep_s: secs(prep),
            recognize_s: recognize.map(secs),
            requests: reqs,
        }
    }
}

/// The page entry of the JSON: a timed page, or one resumed from a checkpoint.
#[derive(Serialize)]
#[serde(untagged)]
enum PageEntry {
    Timed(PageTiming),
    Resumed { page: usize, resumed: bool },
}

#[derive(Serialize)]
struct Totals {
    wall_s: f64,
    prep_s: f64,
    requests_s: f64,
    server_prompt_s: f64,
    server_decode_s: f64,
    prompt_tokens: u64,
    completion_tokens: u64,
    verify_s: f64,
    write_s: f64,
    attempts_by_reason: BTreeMap<&'static str, usize>,
}

#[derive(Serialize)]
struct Report<'a> {
    engine: &'a str,
    model: Option<&'a str>,
    dpi: u32,
    pages: Vec<PageEntry>,
    totals: Totals,
}

/// Per-file figures the writer measured.
pub struct FileTimes {
    pub wall: Duration,
    pub verify: Duration,
    pub write: Duration,
}

/// The `<stem>.timing.json` document. `selected`: the page numbers in order; pages without an
/// entry in `timed` were resumed from a checkpoint.
pub fn report_json(
    engine: &str,
    model: Option<&str>,
    dpi: u32,
    selected: &[usize],
    timed: &BTreeMap<usize, PageTiming>,
    times: &FileTimes,
) -> String {
    let mut t = Totals {
        wall_s: secs(times.wall),
        prep_s: 0.0,
        requests_s: 0.0,
        server_prompt_s: 0.0,
        server_decode_s: 0.0,
        prompt_tokens: 0,
        completion_tokens: 0,
        verify_s: secs(times.verify),
        write_s: secs(times.write),
        attempts_by_reason: BTreeMap::new(),
    };
    let mut pages = Vec::new();
    for &n in selected {
        let Some(p) = timed.get(&n) else {
            pages.push(PageEntry::Resumed {
                page: n,
                resumed: true,
            });
            continue;
        };
        t.prep_s += p.prep_s;
        for r in &p.requests {
            t.requests_s += r.wall_s;
            t.server_prompt_s += r.timings.prompt_ms.unwrap_or(0.0) / 1000.0;
            t.server_decode_s += r.timings.predicted_ms.unwrap_or(0.0) / 1000.0;
            t.prompt_tokens += r.prompt_tokens().unwrap_or(0);
            t.completion_tokens += r.completion_tokens().unwrap_or(0);
            *t.attempts_by_reason.entry(r.reason).or_default() += 1;
        }
        pages.push(PageEntry::Timed(p.clone()));
    }
    for v in [
        &mut t.prep_s,
        &mut t.requests_s,
        &mut t.server_prompt_s,
        &mut t.server_decode_s,
    ] {
        *v = (*v * 1000.0).round() / 1000.0;
    }
    let report = Report {
        engine,
        model,
        dpi,
        pages,
        totals: t,
    };
    serde_json::to_string_pretty(&report).expect("timing report serializes") + "\n"
}

/// `<dir>/<stem>.timing.json` next to the Markdown output.
pub fn sidecar(markdown: &Path) -> PathBuf {
    let stem = markdown
        .file_stem()
        .map_or_else(|| "output".into(), |s| s.to_string_lossy().into_owned());
    markdown.with_file_name(format!("{stem}.timing.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_usage_and_timings() {
        let m = parse_metrics(&json!({
            "usage": {"prompt_tokens": 1260, "completion_tokens": 410, "total_tokens": 1670,
                      "prompt_tokens_details": {"cached_tokens": 12}},
            "timings": {"prompt_n": 1248, "cache_n": 12, "predicted_n": 410, "prompt_ms": 900.5,
                        "predicted_ms": 2300.0, "peak_memory": 4.5}
        }));
        assert_eq!(m.usage.prompt_tokens, Some(1260));
        assert_eq!(m.usage.completion_tokens, Some(410));
        assert_eq!(m.usage.cached_tokens, Some(12));
        assert_eq!(m.timings.prompt_n, Some(1248));
        assert_eq!(m.timings.cache_n, Some(12));
        assert_eq!(m.timings.predicted_n, Some(410));
        assert_eq!(m.timings.prompt_ms, Some(900.5));
        assert_eq!(m.timings.predicted_ms, Some(2300.0));
        assert_eq!(m.timings.peak_memory, Some(4.5));
    }

    #[test]
    fn absent_metrics_are_empty() {
        let m = parse_metrics(&json!({"choices": []}));
        assert_eq!(m, ServerMetrics::default());
    }

    #[test]
    fn partial_and_odd_metrics_keep_what_is_usable() {
        let m = parse_metrics(&json!({
            "usage": {"completion_tokens": 7, "prompt_tokens": "many"},
            "timings": {"predicted_ms": 12.5, "prompt_n": 3.0}
        }));
        assert_eq!(m.usage.completion_tokens, Some(7));
        assert_eq!(m.usage.prompt_tokens, None);
        assert_eq!(m.timings.predicted_ms, Some(12.5));
        assert_eq!(m.timings.prompt_n, Some(3));
        assert_eq!(m.timings.prompt_ms, None);
    }

    #[test]
    fn describe_line() {
        let r = Request {
            label: "OCR:".into(),
            image_bytes: 812 * 1024,
            wall_s: 3.4,
            reason: "first",
            outcome: "ok".into(),
            finish_reason: Some("stop".into()),
            max_tokens: 4096,
            repetition_penalty: None,
            usage: Usage {
                prompt_tokens: Some(1260),
                completion_tokens: Some(410),
                cached_tokens: None,
            },
            timings: Timings {
                prompt_ms: Some(900.0),
                predicted_ms: Some(2300.0),
                ..Timings::default()
            },
        };
        assert_eq!(
            r.describe(),
            "812 KB, 3.4s (prompt 1260 tok 0.9s, decode 410 tok 2.3s)"
        );
    }

    #[test]
    fn reasons_follow_the_previous_attempt() {
        let _ = take();
        assert_eq!(reason(1, 4096, 4096, None), "first");
        let req = |reason| Request {
            label: "x".into(),
            image_bytes: 0,
            wall_s: 0.0,
            reason,
            outcome: "ok".into(),
            finish_reason: None,
            max_tokens: 0,
            repetition_penalty: None,
            usage: Usage::default(),
            timings: Timings::default(),
        };
        record(1, req("first"), Some(true));
        assert_eq!(reason(1, 4096, 4096, None), "empty_retry");
        assert_eq!(reason(1, 8192, 4096, None), "length_retry");
        assert_eq!(reason(1, 4096, 4096, Some(1.1)), "loop_retry");
        assert_eq!(reason(2, 4096, 4096, None), "first");
        assert_eq!(take().len(), 1);
    }
}
