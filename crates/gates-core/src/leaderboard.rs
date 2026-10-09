//! The UGI Leaderboard (huggingface.co/spaces/DontPlanToEnd/UGI-Leaderboard,
//! by DontPlanToEnd): scores for open models, to browse on the Models page
//! before finding a GGUF of the one you pick.
//!
//! The data is one CSV in the Space, which has no API and states no licence.
//! So it is fetched at run time, from the source, and kept only in the
//! user's cache (`$XDG_CACHE_HOME/telamon-gates/ugi.csv`); it is never bundled
//! or redistributed, and the page names and links the source.
//!
//! The file is untrusted: size capped, parsed by columns' names (not their
//! places), bad rows skipped, row count capped, strings cleaned of control
//! characters and cut short, and a link kept only when it is a
//! `https://huggingface.co/` address. Rows without parameters (the
//! proprietary models) are dropped: there are no weights to get.
//!
//! Everything here blocks on the network or the disk: call it from a worker.

use serde::Deserialize;
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The CSV, which the Space re-uploads in place.
pub const DATA_URL: &str = "https://huggingface.co/spaces/DontPlanToEnd/UGI-Leaderboard/resolve/main/ugi-leaderboard-data.csv";
/// The Space, where the page's attribution links.
pub const SPACE_URL: &str = "https://huggingface.co/spaces/DontPlanToEnd/UGI-Leaderboard";
/// A copy newer than this is used without asking the Hub.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// The most the answer (or the cache file) may be.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// More rows than this and the rest are ignored (the file has about 1,300).
pub const MAX_ROWS: usize = 5000;
/// The longest a field kept from the file is, in characters.
const FIELD_MAX: usize = 200;
const TIMEOUT: Duration = Duration::from_secs(30);

/// What a model is, from the leaderboard's flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Base,
    Finetune,
    Merge,
    Unknown,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Base => "Base",
            Kind::Finetune => "Finetune",
            Kind::Merge => "Merge",
            Kind::Unknown => "",
        }
    }

    /// From the page's word; `None` for "any".
    pub fn from_label(label: &str) -> Option<Kind> {
        match label {
            "Base" => Some(Kind::Base),
            "Finetune" => Some(Kind::Finetune),
            "Merge" => Some(Kind::Merge),
            _ => None,
        }
    }
}

/// One open model on the leaderboard.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// `author/model_name`, as the leaderboard writes it (some carry a
    /// setting in brackets, "(reasoning=low)").
    pub name: String,
    /// The model's page, only when it is on `https://huggingface.co/`.
    pub link: Option<String>,
    /// As `yyyymmdd`; 0 when unknown.
    pub released: u32,
    /// Billions of parameters.
    pub total: f64,
    pub active: Option<f64>,
    pub kind: Kind,
    pub thinking: bool,
    /// Overall (0 to 100).
    pub ugi: Option<f64>,
    /// Willingness (0 to 10).
    pub willingness: Option<f64>,
    /// Knowledge: NatInt (0 to 100).
    pub knowledge: Option<f64>,
    pub writing: Option<f64>,
}

impl Entry {
    /// A mixture of experts: fewer parameters at work than it holds.
    pub fn experts(&self) -> bool {
        self.active.is_some_and(|a| a < self.total)
    }

    /// About what a Q4 file takes: 0.6 GB for each billion parameters.
    pub fn estimated_bytes(&self) -> f64 {
        self.total * 0.6e9
    }

    pub fn fit(&self, vram: f64) -> Option<Fit> {
        (vram > 0.0).then(|| fit(self.estimated_bytes(), vram, self.experts()))
    }

    /// What to search Hugging Face for to find a GGUF of this model: its
    /// repository's name from the link (else the leaderboard's name, without
    /// the author and any bracketed setting), and "GGUF".
    pub fn gguf_query(&self) -> String {
        let from_link = self.link.as_deref().and_then(|l| {
            let url = url::Url::parse(l).ok()?;
            url.path_segments()?.nth(1).map(str::to_string)
        });
        let name = from_link.unwrap_or_else(|| {
            let name = self.name.rsplit('/').next().unwrap_or(&self.name);
            name.split(" (").next().unwrap_or(name).to_string()
        });
        let name: String = name
            .chars()
            .filter(|c| c.is_alphanumeric() || "-_. ".contains(*c))
            .collect();
        format!("{} GGUF", name.trim())
    }
}

/// How a model sits in the graphics card's memory: room to spare, tight
/// (some layers may run on the processor) or too big (most will). The same
/// rule as `ModelsPage.qml`'s `fit`, with mixtures of experts, which still
/// run when part of them is in system memory, never too big.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Fits,
    Tight,
    Big,
}

pub fn fit(bytes: f64, vram: f64, experts: bool) -> Fit {
    let plain = if bytes * 1.2 <= vram {
        Fit::Fits
    } else if bytes <= vram {
        Fit::Tight
    } else {
        Fit::Big
    };
    if experts && plain == Fit::Big {
        Fit::Tight
    } else {
        plain
    }
}

impl Fit {
    pub fn word(self) -> &'static str {
        match self {
            Fit::Fits => "fits",
            Fit::Tight => "tight",
            Fit::Big => "big",
        }
    }
}

/// The open models of a leaderboard file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub entries: Vec<Entry>,
    /// Rows left out: damaged, nameless, or without parameters (proprietary).
    pub skipped: usize,
}

// ---------------------------------------------------------------------
// Parsing

/// The CSV's records. Quotes may hold commas, line breaks and `""`; a field
/// is cut at `FIELD_MAX` characters (the rest is read and dropped) and at
/// most `max` records are read.
fn records(text: &str, max: usize) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut kept = 0usize;
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let push = |field: &mut String, kept: &mut usize, row: &mut Vec<String>| {
        row.push(std::mem::take(field));
        *kept = 0;
    };
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    if kept < FIELD_MAX {
                        field.push('"');
                        kept += 1;
                    }
                }
                '"' => quoted = false,
                _ if kept < FIELD_MAX => {
                    field.push(c);
                    kept += 1;
                }
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => push(&mut field, &mut kept, &mut row),
            '\r' => {}
            '\n' => {
                push(&mut field, &mut kept, &mut row);
                out.push(std::mem::take(&mut row));
                if out.len() >= max {
                    return out;
                }
            }
            _ if kept < FIELD_MAX => {
                field.push(c);
                kept += 1;
            }
            _ => {}
        }
    }
    if !field.is_empty() || !row.is_empty() {
        push(&mut field, &mut kept, &mut row);
        out.push(row);
    }
    out
}

/// A header as a key: the emoji and other marks dropped, lowercase
/// ("UGI 🏆" is "ugi", "W/10 👍" is "w/10", and "UGI non-W/10" stays apart).
fn header_key(header: &str) -> String {
    header
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "/-_ ".contains(*c))
        .collect::<String>()
        .trim()
        .to_ascii_lowercase()
}

/// A field cleaned for display: no control or direction-changing characters,
/// trimmed, at most `max` characters.
pub fn clean(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !('\u{202a}'..='\u{202e}').contains(c))
        .filter(|c| !('\u{2066}'..='\u{2069}').contains(c) && *c != '\u{200e}' && *c != '\u{200f}')
        .collect::<String>()
        .trim()
        .chars()
        .take(max)
        .collect()
}

/// A number in `0..=max`; `None` for "NA", empty, or anything else.
fn number(text: &str, max: f64) -> Option<f64> {
    let n: f64 = text.trim().parse().ok()?;
    (n.is_finite() && (0.0..=max).contains(&n)).then_some(n)
}

fn flag(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("true")
}

/// `M/D/YYYY` as `yyyymmdd`.
fn date(text: &str) -> u32 {
    let mut parts = text.trim().split('/').map(|p| p.trim().parse::<u32>());
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(m)), Some(Ok(d)), Some(Ok(y)), None)
            if (1..=12).contains(&m) && (1..=31).contains(&d) && (1990..=2100).contains(&y) =>
        {
            y * 10000 + m * 100 + d
        }
        _ => 0,
    }
}

/// `https://huggingface.co/...` and nothing else: no other host, scheme,
/// port or credentials.
pub fn hub_link(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() > FIELD_MAX {
        return None;
    }
    let url = url::Url::parse(text).ok()?;
    let ok = url.scheme() == "https"
        && url.host_str() == Some("huggingface.co")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path().len() > 1;
    ok.then(|| url.to_string())
}

/// The open models in `text`, the leaderboard's CSV.
pub fn parse(text: &str) -> Result<Table, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut records = records(text, MAX_ROWS + 1).into_iter();
    let header: Vec<String> = records
        .next()
        .ok_or("The leaderboard's file is empty.")?
        .iter()
        .map(|h| header_key(h))
        .collect();
    let col = |name: &str| header.iter().position(|h| h == name);
    let (Some(name_col), Some(total_col)) = (col("author/model_name"), col("total parameters"))
    else {
        return Err("The leaderboard's file isn't the table Gates reads.".into());
    };
    let (link, released, active) = (
        col("model link"),
        col("release date"),
        col("active parameters"),
    );
    let (finetuned, merged, foundation, thinking) = (
        col("is finetuned"),
        col("is merged"),
        col("is foundation"),
        col("is thinking model"),
    );
    let (ugi, willingness, knowledge, writing) =
        (col("ugi"), col("w/10"), col("natint"), col("writing"));

    let mut table = Table::default();
    for record in records {
        let get = |c: Option<usize>| c.and_then(|c| record.get(c)).map_or("", String::as_str);
        let name = clean(record.get(name_col).map_or("", String::as_str), FIELD_MAX);
        let total = number(get(Some(total_col)), 100_000.0).filter(|t| *t > 0.0);
        let (Some(total), false) = (total, name.is_empty()) else {
            table.skipped += 1;
            continue;
        };
        let kind = if flag(get(merged)) {
            Kind::Merge
        } else if flag(get(finetuned)) {
            Kind::Finetune
        } else if flag(get(foundation)) {
            Kind::Base
        } else {
            Kind::Unknown
        };
        table.entries.push(Entry {
            name,
            link: hub_link(get(link)),
            released: date(get(released)),
            total,
            active: number(get(active), 100_000.0).filter(|a| *a > 0.0),
            kind,
            thinking: flag(get(thinking)),
            ugi: number(get(ugi), 100.0),
            willingness: number(get(willingness), 10.0),
            knowledge: number(get(knowledge), 100.0),
            writing: number(get(writing), 100.0),
        });
    }
    if table.entries.is_empty() {
        return Err("The leaderboard's file has no models Gates can read.".into());
    }
    Ok(table)
}

// ---------------------------------------------------------------------
// Filtering and sorting

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Ugi,
    Writing,
    Knowledge,
    Willingness,
    Newest,
}

impl Sort {
    /// From the page's word; the overall score for anything else.
    pub fn from_word(word: &str) -> Sort {
        match word {
            "writing" => Sort::Writing,
            "knowledge" => Sort::Knowledge,
            "willingness" => Sort::Willingness,
            "newest" => Sort::Newest,
            _ => Sort::Ugi,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Filter {
    /// Every word must be in the name.
    pub search: String,
    pub kind: Option<Kind>,
    pub hide_thinking: bool,
    /// 0 for any; a model without the score is then left out.
    pub min_willingness: f64,
    /// The graphics card's memory in bytes: models too big for it are left
    /// out. `None` (or 0) when not asked for or not known.
    pub fits_vram: Option<f64>,
    pub sort: Sort,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            search: String::new(),
            kind: None,
            hide_thinking: false,
            min_willingness: 0.0,
            fits_vram: None,
            sort: Sort::Ugi,
        }
    }
}

impl Filter {
    fn keeps(&self, e: &Entry, words: &[String]) -> bool {
        if self.kind.is_some_and(|k| e.kind != k) || (self.hide_thinking && e.thinking) {
            return false;
        }
        if self.min_willingness > 0.0 && !e.willingness.is_some_and(|w| w >= self.min_willingness) {
            return false;
        }
        if let Some(vram) = self.fits_vram.filter(|v| *v > 0.0)
            && fit(e.estimated_bytes(), vram, e.experts()) == Fit::Big
        {
            return false;
        }
        let name = e.name.to_lowercase();
        words.iter().all(|w| name.contains(w.as_str()))
    }
}

/// The entries that pass `filter`, best first: the models without the score
/// sorted by last, ties by name.
pub fn select<'a>(entries: &'a [Entry], filter: &Filter) -> Vec<&'a Entry> {
    let words: Vec<String> = filter
        .search
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let mut kept: Vec<&Entry> = entries.iter().filter(|e| filter.keeps(e, &words)).collect();
    let key = |e: &Entry| -> f64 {
        match filter.sort {
            Sort::Ugi => e.ugi,
            Sort::Writing => e.writing,
            Sort::Knowledge => e.knowledge,
            Sort::Willingness => e.willingness,
            Sort::Newest => (e.released > 0).then_some(f64::from(e.released)),
        }
        .unwrap_or(f64::NEG_INFINITY)
    };
    kept.sort_by(|a, b| {
        key(b)
            .total_cmp(&key(a))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    kept
}

// ---------------------------------------------------------------------
// What the Models page shows

/// A parameter count as models name it: "8B", "0.5B", "70B".
fn billions(n: f64) -> String {
    let text = if n >= 10.0 || (n - n.round()).abs() < 0.05 {
        format!("{n:.0}")
    } else {
        format!("{n:.1}")
    };
    format!("{text}B")
}

/// `yyyymmdd` as "Mar 17, 2025"; "" for 0.
fn release_label(released: u32) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if released == 0 {
        return String::new();
    }
    let (y, m, d) = (released / 10000, released / 100 % 100, released % 100);
    format!("{} {d}, {y}", MONTHS[(m as usize).clamp(1, 12) - 1])
}

/// What the page asks for, as the JSON QML sends it.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Spec {
    search: String,
    /// "Base", "Finetune" or "Merge"; anything else is any.
    kind: String,
    hide_thinking: bool,
    min_willingness: f64,
    /// The card's bytes, to hide the models too big for it; 0 for no filter.
    fits_vram: f64,
    /// The card's bytes for each row's fit badge; 0 when unknown.
    vram: f64,
    /// "ugi", "writing", "knowledge", "willingness" or "newest".
    sort: String,
    limit: usize,
}

/// The models that pass the filters in `spec` (JSON, see `Spec`), best
/// first and at most `limit` (never over 500), as `rows_json` writes them.
pub fn query(entries: &[Entry], spec: &str) -> String {
    let spec: Spec = serde_json::from_str(spec).unwrap_or_default();
    let filter = Filter {
        search: clean(&spec.search, 100),
        kind: Kind::from_label(&spec.kind),
        hide_thinking: spec.hide_thinking,
        min_willingness: if spec.min_willingness.is_finite() {
            spec.min_willingness
        } else {
            0.0
        },
        fits_vram: (spec.fits_vram.is_finite() && spec.fits_vram > 0.0).then_some(spec.fits_vram),
        sort: Sort::from_word(&spec.sort),
    };
    let found = select(entries, &filter);
    let shown = &found[..found.len().min(spec.limit.min(500))];
    let vram = if spec.vram.is_finite() {
        spec.vram.max(0.0)
    } else {
        0.0
    };
    rows_json(found.len(), shown, vram)
}

/// The rows for the page, as JSON for QML (`JSON.parse`): `{"total": n,
/// "rows": [...]}`, `total` being how many passed the filters. The strings
/// are the file's, so QML shows them as plain text.
pub fn rows_json(total: usize, rows: &[&Entry], vram: f64) -> String {
    let score = |s: Option<f64>| s.map_or(Value::Null, |s| json!(s));
    let rows: Vec<Value> = rows
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "query": e.gguf_query(),
                "link": e.link.clone().unwrap_or_default(),
                "ugi": score(e.ugi),
                "willingness": score(e.willingness),
                "knowledge": score(e.knowledge),
                "writing": score(e.writing),
                "params": billions(e.total),
                "active": if e.experts() { e.active.map(billions).unwrap_or_default() } else { String::new() },
                "kind": e.kind.label(),
                "thinking": e.thinking,
                "released": release_label(e.released),
                "bytes": e.estimated_bytes(),
                "fit": e.fit(vram).map_or("", Fit::word),
            })
        })
        .collect();
    json!({ "total": total, "rows": rows }).to_string()
}

// ---------------------------------------------------------------------
// The cached copy

/// `$XDG_CACHE_HOME/telamon-gates` (`~/.cache/telamon-gates`).
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
        .join("telamon-gates")
}

/// Where the copy is kept.
pub fn cache_path() -> PathBuf {
    cache_dir().join("ugi.csv")
}

fn etag_path(cache: &Path) -> PathBuf {
    cache.with_extension("etag")
}

/// The leaderboard, and how fresh it is.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub table: Table,
    /// When the copy was last known to be what the Space has.
    pub fetched: SystemTime,
    /// Something to tell the user, such as a stale copy kept after the
    /// Hub couldn't be reached.
    pub note: Option<String>,
}

/// What the Hub answered to a conditional request.
pub enum Reply {
    NotModified,
    Body { text: String, etag: Option<String> },
}

fn read_capped(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("too big"));
    }
    String::from_utf8(bytes).map_err(io::Error::other)
}

/// The cached copy, however old; `None` when there is none or it can't be
/// read. Never touches the network.
pub fn cached(cache: &Path) -> Option<Loaded> {
    let fetched = fs::metadata(cache).ok()?.modified().ok()?;
    let table = parse(&read_capped(cache).ok()?).ok()?;
    Some(Loaded {
        table,
        fetched,
        note: None,
    })
}

/// The leaderboard: the cached copy when it is under 24 hours old (and
/// `force` is off), else what the Space has now. `fetch` asks the Hub, given
/// the ETag of the copy held; the real one is `fetch_from_hub`.
pub fn load_with(
    cache: &Path,
    force: bool,
    now: SystemTime,
    fetch: &dyn Fn(Option<&str>) -> Result<Reply, String>,
) -> Result<Loaded, String> {
    let held = cached(cache);
    if !force
        && let Some(held) = &held
        && now.duration_since(held.fetched).unwrap_or_default() < MAX_AGE
    {
        return Ok(held.clone());
    }
    let etag = held
        .as_ref()
        .and_then(|_| fs::read_to_string(etag_path(cache)).ok())
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty() && e.len() <= 200 && e.is_ascii());
    match fetch(etag.as_deref()) {
        Ok(Reply::NotModified) => match held {
            Some(mut held) => {
                // Still what the Space has: fresh for another day.
                if let Err(e) = File::options()
                    .write(true)
                    .open(cache)
                    .and_then(|f| f.set_modified(now))
                {
                    log::warn!("couldn't touch {}: {e}", cache.display());
                }
                held.fetched = now;
                Ok(held)
            }
            None => Err("Hugging Face said nothing changed, but there's no saved copy.".into()),
        },
        Ok(Reply::Body { text, etag }) => {
            // Kept only when it reads: a damaged file never replaces a good one.
            let table = parse(&text)?;
            write_private(cache, text.as_bytes()).map_err(|e| {
                format!("Couldn't save the leaderboard to {}: {e}.", cache.display())
            })?;
            let tag_path = etag_path(cache);
            match etag.filter(|e| e.len() <= 200 && e.is_ascii()) {
                Some(tag) => {
                    let _ = write_private(&tag_path, tag.as_bytes());
                }
                None => {
                    let _ = fs::remove_file(&tag_path);
                }
            }
            Ok(Loaded {
                table,
                fetched: now,
                note: None,
            })
        }
        Err(e) => match held {
            Some(mut held) => {
                held.note = Some(format!("{e} Showing the saved copy."));
                Ok(held)
            }
            None => Err(e),
        },
    }
}

/// `load_with` against the real Hub and the user's cache.
pub fn load(force: bool) -> Result<Loaded, String> {
    load_with(&cache_path(), force, SystemTime::now(), &fetch_from_hub)
}

/// Writes `bytes` to `path` through a private temporary file and a rename,
/// in a private folder.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        crate::store::private_dir(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
    }
    fs::rename(&tmp, path)
}

/// Asks the Hub for the CSV, with `If-None-Match` when an ETag is held.
/// https only, a timeout, and at most `MAX_BYTES`.
pub fn fetch_from_hub(etag: Option<&str>) -> Result<Reply, String> {
    let agent = crate::hub::https_agent();
    let mut request = agent
        .get(DATA_URL)
        .config()
        .timeout_global(Some(TIMEOUT))
        .build();
    if let Some(tag) = etag {
        request = request.header("If-None-Match", format!("\"{tag}\""));
    }
    let response = request
        .call()
        .map_err(|e| format!("Couldn't reach Hugging Face: {e}."))?;
    match response.status().as_u16() {
        304 => return Ok(Reply::NotModified),
        200 => {}
        status => return Err(format!("Hugging Face answered {status}.")),
    }
    let tag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.trim()
                .trim_start_matches("W/")
                .trim_matches('"')
                .to_string()
        });
    let text = response
        .into_body()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_string()
        .map_err(|e| format!("Couldn't read the leaderboard: {e}."))?;
    Ok(Reply::Body { text, etag: tag })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-written file in the real one's shape: a byte order mark, emoji
    /// in the headers, a column that starts like another ("UGI non-W/10"),
    /// quoted names with commas, NA, a proprietary row, a damaged one.
    const SAMPLE: &str = "\u{feff}author/model_name,Model Link,Release Date,Active Parameters,Total Parameters,Is Finetuned,Is Merged,Is Foundation,Writing ✍️,UGI 🏆,UGI non-W/10,W/10 👍,NatInt 💡,Is Thinking Model,Architecture\r\n\
        acme/Alpha-8B,https://huggingface.co/acme/Alpha-8B,3/17/2025,8.0,8.0,FALSE,FALSE,TRUE,40.5,50.25,48.0,6.5,30.0,FALSE,LlamaForCausalLM\r\n\
        acme/Beta-30B-A3B,https://huggingface.co/acme/Beta-30B-A3B,10/2/2025,3.0,30.0,TRUE,FALSE,FALSE,55.0,60.0,58.0,8.0,45.5,TRUE,Qwen3MoeForCausalLM\r\n\
        \"weird/Name, with comma\",https://evil.example/x,1/5/2024,NA,13.0,FALSE,TRUE,FALSE,NA,NA,NA,NA,NA,FALSE,\r\n\
        openai/gpt-closed (reasoning=low),https://huggingface.co/openai/gpt-closed,8/4/2025,,,FALSE,FALSE,TRUE,10,20,18,2,5,TRUE,\r\n\
        ,https://huggingface.co/x/y,1/1/2025,1,1,FALSE,FALSE,TRUE,1,1,1,1,1,FALSE,\r\n\
        acme/Gamma-70B,http://huggingface.co/acme/Gamma-70B,12/25/2024,70.0,70.0,FALSE,FALSE,TRUE,30,35,33,9.5,38,FALSE,x\r\n";

    fn sample() -> Table {
        parse(SAMPLE).unwrap()
    }

    fn entry<'a>(t: &'a Table, name: &str) -> &'a Entry {
        t.entries.iter().find(|e| e.name == name).unwrap()
    }

    #[test]
    fn parses_the_columns_by_name_past_the_bom_and_emoji() {
        let t = sample();
        // The proprietary row (no total parameters) and the nameless one go.
        assert_eq!(t.entries.len(), 4);
        assert_eq!(t.skipped, 2);
        let a = entry(&t, "acme/Alpha-8B");
        assert_eq!(a.ugi, Some(50.25));
        assert_eq!(a.willingness, Some(6.5));
        assert_eq!(a.knowledge, Some(30.0));
        assert_eq!(a.writing, Some(40.5));
        assert_eq!(a.total, 8.0);
        assert_eq!(a.released, 20250317);
        assert_eq!(a.kind, Kind::Base);
        assert!(!a.thinking && !a.experts());
        assert_eq!(
            a.link.as_deref(),
            Some("https://huggingface.co/acme/Alpha-8B")
        );
        let b = entry(&t, "acme/Beta-30B-A3B");
        assert_eq!(
            (b.kind, b.thinking, b.experts()),
            (Kind::Finetune, true, true)
        );
        assert_eq!(b.released, 20251002);
    }

    #[test]
    fn na_is_missing_and_quotes_hold_commas() {
        let t = sample();
        let w = entry(&t, "weird/Name, with comma");
        assert_eq!(
            (w.ugi, w.willingness, w.knowledge, w.writing),
            (None, None, None, None)
        );
        assert_eq!(w.kind, Kind::Merge);
        assert_eq!(w.active, None);
    }

    #[test]
    fn links_are_kept_only_on_the_hub_over_https() {
        let t = sample();
        assert_eq!(entry(&t, "weird/Name, with comma").link, None);
        assert_eq!(entry(&t, "acme/Gamma-70B").link, None);
        for bad in [
            "https://huggingface.co.evil.example/a/b",
            "https://user@huggingface.co/a/b",
            "https://huggingface.co:8443/a/b",
            "javascript:alert(1)",
            "https://huggingface.co/",
            "file:///etc/passwd",
            "",
        ] {
            assert_eq!(hub_link(bad), None, "{bad}");
        }
        assert!(hub_link("https://huggingface.co/a/b").is_some());
    }

    #[test]
    fn the_headers_are_matched_without_their_emoji() {
        assert_eq!(header_key("UGI 🏆"), "ugi");
        assert_eq!(header_key("W/10 👍"), "w/10");
        assert_eq!(header_key("NatInt 💡"), "natint");
        assert_eq!(header_key("Writing ✍️"), "writing");
        assert_eq!(header_key("UGI non-W/10"), "ugi non-w/10");
        // A different emoji, or none, finds the same column.
        let t = parse("author/model_name,Total Parameters,UGI,W/10 🔥\nx/y,7,12.5,3\n").unwrap();
        assert_eq!(t.entries[0].ugi, Some(12.5));
        assert_eq!(t.entries[0].willingness, Some(3.0));
    }

    #[test]
    fn a_file_that_isnt_the_table_is_refused() {
        assert!(parse("").is_err());
        assert!(parse("<html>nope</html>").is_err());
        assert!(parse("author/model_name,Total Parameters\n").is_err());
        assert!(parse("a,b\n1,2\n").is_err());
    }

    #[test]
    fn strings_are_cleaned_and_capped() {
        assert_eq!(clean("  a\u{7}b\u{202e}c\n", 10), "abc");
        let long = format!(
            "author/model_name,Total Parameters\n{},7\n",
            "x".repeat(5000)
        );
        let t = parse(&long).unwrap();
        assert_eq!(t.entries[0].name.chars().count(), FIELD_MAX);
    }

    #[test]
    fn rows_and_scores_out_of_range_are_dropped() {
        let t = parse(
            "author/model_name,Total Parameters,UGI,W/10\na/b,-3,10,1\nc/d,7,500,99\ne/f,inf,1,1\ng/h,1e9,1,1\n",
        );
        // Only c/d has parameters in range; its scores are out of range.
        let t = t.unwrap();
        assert_eq!(t.entries.len(), 1);
        assert_eq!((t.entries[0].ugi, t.entries[0].willingness), (None, None));
    }

    #[test]
    fn the_row_count_is_capped() {
        let mut text = String::from("author/model_name,Total Parameters\n");
        for i in 0..(MAX_ROWS + 500) {
            text.push_str(&format!("a/m{i},7\n"));
        }
        assert_eq!(parse(&text).unwrap().entries.len(), MAX_ROWS);
    }

    #[test]
    fn a_last_line_without_a_newline_counts() {
        let t = parse("author/model_name,Total Parameters\nx/y,7").unwrap();
        assert_eq!(t.entries.len(), 1);
    }

    #[test]
    fn filters_and_sorts() {
        let t = sample();
        let names = |f: &Filter| -> Vec<String> {
            select(&t.entries, f)
                .iter()
                .map(|e| e.name.clone())
                .collect()
        };
        let all = names(&Filter::default());
        assert_eq!(all[0], "acme/Beta-30B-A3B");
        assert_eq!(all[1], "acme/Alpha-8B");
        // No score: last.
        assert_eq!(all[3], "weird/Name, with comma");

        let f = Filter {
            kind: Some(Kind::Base),
            ..Default::default()
        };
        assert_eq!(names(&f), ["acme/Alpha-8B", "acme/Gamma-70B"]);
        let f = Filter {
            hide_thinking: true,
            ..Default::default()
        };
        assert_eq!(names(&f).len(), 3);
        let f = Filter {
            min_willingness: 7.0,
            ..Default::default()
        };
        assert_eq!(names(&f), ["acme/Beta-30B-A3B", "acme/Gamma-70B"]);
        let f = Filter {
            search: "ALPHA 8b".into(),
            ..Default::default()
        };
        assert_eq!(names(&f), ["acme/Alpha-8B"]);
        let f = Filter {
            sort: Sort::Newest,
            ..Default::default()
        };
        assert_eq!(names(&f)[0], "acme/Beta-30B-A3B");
        let f = Filter {
            sort: Sort::Willingness,
            ..Default::default()
        };
        assert_eq!(names(&f)[0], "acme/Gamma-70B");
        let f = Filter {
            sort: Sort::Knowledge,
            ..Default::default()
        };
        assert_eq!(names(&f)[0], "acme/Beta-30B-A3B");
        let f = Filter {
            sort: Sort::Writing,
            ..Default::default()
        };
        assert_eq!(names(&f)[0], "acme/Beta-30B-A3B");
    }

    #[test]
    fn fits_my_graphics_card_hides_too_big_but_keeps_experts() {
        let t = sample();
        let gib = 1024.0 * 1024.0 * 1024.0;
        // 16 GiB: the 8B (4.8 GB) fits, the 30B A3B (18 GB) is a mixture of
        // experts, so tight; the 13B merge (7.8 GB) fits; the 70B (42 GB) is
        // too big.
        let f = Filter {
            fits_vram: Some(16.0 * gib),
            ..Default::default()
        };
        let names: Vec<_> = select(&t.entries, &f)
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert!(names.contains(&"acme/Beta-30B-A3B".to_string()));
        assert!(!names.contains(&"acme/Gamma-70B".to_string()));
        assert_eq!(names.len(), 3);
        // Not known: nothing is hidden.
        let f = Filter {
            fits_vram: Some(0.0),
            ..Default::default()
        };
        assert_eq!(select(&t.entries, &f).len(), 4);
    }

    #[test]
    fn fit_is_the_models_pages_rule() {
        let gib = 1024.0 * 1024.0 * 1024.0;
        assert_eq!(fit(8.0 * gib, 16.0 * gib, false), Fit::Fits);
        assert_eq!(fit(14.0 * gib, 16.0 * gib, false), Fit::Tight);
        assert_eq!(fit(20.0 * gib, 16.0 * gib, false), Fit::Big);
        assert_eq!(fit(20.0 * gib, 16.0 * gib, true), Fit::Tight);
        assert_eq!(fit(8.0 * gib, 16.0 * gib, true), Fit::Fits);
    }

    #[test]
    fn gguf_queries_use_the_repository_name() {
        let t = sample();
        assert_eq!(entry(&t, "acme/Alpha-8B").gguf_query(), "Alpha-8B GGUF");
        // No link: the name, without the author.
        assert_eq!(
            entry(&t, "weird/Name, with comma").gguf_query(),
            "Name with comma GGUF"
        );
        let e = Entry {
            name: "openai/gpt-oss-20b (reasoning=low)".into(),
            link: None,
            released: 0,
            total: 20.0,
            active: None,
            kind: Kind::Base,
            thinking: false,
            ugi: None,
            willingness: None,
            knowledge: None,
            writing: None,
        };
        assert_eq!(e.gguf_query(), "gpt-oss-20b GGUF");
    }

    #[test]
    fn the_rows_for_the_page() {
        let t = sample();
        let gib = 1024.0 * 1024.0 * 1024.0;
        let rows = select(&t.entries, &Filter::default());
        let json: Value = serde_json::from_str(&rows_json(4, &rows[..2], 16.0 * gib)).unwrap();
        assert_eq!(json["total"], 4);
        let beta = &json["rows"][0];
        assert_eq!(beta["name"], "acme/Beta-30B-A3B");
        assert_eq!(beta["params"], "30B");
        assert_eq!(beta["active"], "3B");
        assert_eq!(beta["fit"], "tight");
        assert_eq!(beta["released"], "Oct 2, 2025");
        assert_eq!(beta["kind"], "Finetune");
        // No card: no fit.
        let json: Value = serde_json::from_str(&rows_json(4, &rows[..1], 0.0)).unwrap();
        assert_eq!(json["rows"][0]["fit"], "");
        assert_eq!(billions(0.5), "0.5B");
        assert_eq!(billions(8.0), "8B");
        assert_eq!(billions(1.3), "1.3B");
    }

    #[test]
    fn the_page_asks_with_json() {
        let t = sample();
        let ask = |spec: &str| -> Value { serde_json::from_str(&query(&t.entries, spec)).unwrap() };
        let all = ask(r#"{"limit": 2}"#);
        assert_eq!(all["total"], 4);
        assert_eq!(all["rows"].as_array().unwrap().len(), 2);
        assert_eq!(all["rows"][0]["name"], "acme/Beta-30B-A3B");
        let some = ask(
            r#"{"search":"acme","kind":"Base","hideThinking":true,"minWillingness":7,"sort":"newest","limit":50,"vram":17179869184}"#,
        );
        assert_eq!(some["total"], 1);
        assert_eq!(some["rows"][0]["name"], "acme/Gamma-70B");
        assert_eq!(some["rows"][0]["fit"], "big");
        let fits = ask(r#"{"fitsVram": 17179869184, "limit": 50}"#);
        assert_eq!(fits["total"], 3);
        // Nonsense asks for nothing, and does no harm.
        assert_eq!(ask("not json")["rows"].as_array().unwrap().len(), 0);
        assert_eq!(
            ask(r#"{"limit": 99999}"#)["rows"].as_array().unwrap().len(),
            4
        );
    }

    // The cache.

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gates-ugi-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("ugi.csv")
    }

    fn body(etag: Option<&str>) -> Result<Reply, String> {
        Ok(Reply::Body {
            text: SAMPLE.to_string(),
            etag: etag.map(str::to_string),
        })
    }

    #[test]
    fn the_first_load_fetches_and_saves() {
        let cache = scratch("first");
        let now = SystemTime::now();
        let got = load_with(&cache, false, now, &|etag| {
            assert_eq!(etag, None);
            body(Some("abc"))
        })
        .unwrap();
        assert_eq!(got.table.entries.len(), 4);
        assert!(cache.exists());
        assert_eq!(fs::read_to_string(etag_path(&cache)).unwrap(), "abc");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(cached(&cache).is_some());
    }

    #[test]
    fn a_fresh_copy_is_used_without_the_network() {
        let cache = scratch("fresh");
        let now = SystemTime::now();
        load_with(&cache, false, now, &|_| body(None)).unwrap();
        let got = load_with(&cache, false, now + Duration::from_secs(3600), &|_| {
            panic!("asked the Hub")
        })
        .unwrap();
        assert_eq!(got.table.entries.len(), 4);
        assert!(got.note.is_none());
    }

    #[test]
    fn an_old_copy_is_checked_with_its_etag_and_kept_on_not_modified() {
        let cache = scratch("old");
        let past = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
        load_with(&cache, false, past, &|_| body(Some("v1"))).unwrap();
        // The file's own time is what the age reads: make it old.
        File::options()
            .write(true)
            .open(&cache)
            .unwrap()
            .set_modified(past)
            .unwrap();
        let now = SystemTime::now();
        let got = load_with(&cache, false, now, &|etag| {
            assert_eq!(etag, Some("v1"));
            Ok(Reply::NotModified)
        })
        .unwrap();
        assert_eq!(got.table.entries.len(), 4);
        // Fresh again: the next load doesn't ask.
        let again = load_with(&cache, false, now, &|_| panic!("asked the Hub")).unwrap();
        assert!(again.fetched >= now - Duration::from_secs(5));
    }

    #[test]
    fn refresh_asks_even_when_fresh() {
        let cache = scratch("force");
        let now = SystemTime::now();
        load_with(&cache, false, now, &|_| body(Some("v1"))).unwrap();
        let asked = std::cell::Cell::new(false);
        load_with(&cache, true, now, &|etag| {
            asked.set(true);
            assert_eq!(etag, Some("v1"));
            Ok(Reply::NotModified)
        })
        .unwrap();
        assert!(asked.get());
    }

    #[test]
    fn a_damaged_answer_never_replaces_a_good_copy() {
        let cache = scratch("damaged");
        let now = SystemTime::now();
        load_with(&cache, false, now, &|_| body(Some("v1"))).unwrap();
        let err = load_with(&cache, true, now, &|_| {
            Ok(Reply::Body {
                text: "<html>captive portal</html>".into(),
                etag: None,
            })
        })
        .unwrap_err();
        assert!(err.contains("isn't the table"), "{err}");
        assert!(cached(&cache).is_some());
    }

    #[test]
    fn when_the_hub_is_down_a_stale_copy_is_shown_with_a_note() {
        let cache = scratch("down");
        let past = SystemTime::now() - Duration::from_secs(5 * 24 * 3600);
        load_with(&cache, false, past, &|_| body(None)).unwrap();
        File::options()
            .write(true)
            .open(&cache)
            .unwrap()
            .set_modified(past)
            .unwrap();
        let got = load_with(&cache, false, SystemTime::now(), &|_| {
            Err("Couldn't reach Hugging Face: offline.".into())
        })
        .unwrap();
        assert_eq!(got.table.entries.len(), 4);
        assert!(got.note.unwrap().contains("Showing the saved copy"));
        // With no copy at all it is an error.
        let none = scratch("none");
        let err = load_with(&none, false, SystemTime::now(), &|_| Err("offline".into()));
        assert_eq!(err.unwrap_err(), "offline");
    }

    #[test]
    fn a_cache_file_too_big_or_not_text_is_ignored() {
        let cache = scratch("huge");
        fs::write(&cache, vec![b'a'; (MAX_BYTES + 10) as usize]).unwrap();
        assert!(cached(&cache).is_none());
        fs::write(&cache, [0xff, 0xfe, 0x00]).unwrap();
        assert!(cached(&cache).is_none());
    }
}
