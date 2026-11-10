//! The documentation MCP tools, answered from the docs built into the binary.
//!
//! `sigiro mcp` proxies `search_docs`, `get_page`, `list_pages` and
//! `get_navigation` to the hosted docs MCP (`SIGIRO_DOCS_MCP_URL`,
//! sigiro.com/mcp/docs). When that endpoint cannot be reached — no network, a
//! timeout, an error — the same four tools answer from here instead
//! (`src/mcp.rs` decides which).
//!
//! The data is Blume's own MCP snapshot of the docs, exported at build time by
//! `www/scripts/export-mcp-data.ts` from the sigiro.com config, so routes and
//! URLs are the hosted ones. The tool definitions (names, titles, descriptions,
//! input schemas) are copied verbatim from Blume's `tools/list` over that data,
//! and the answers follow Blume's `ai/mcp/query.ts` field for field:
//! `get_page`, `list_pages` and `get_navigation` return the same text as the
//! hosted tools. `search_docs` reimplements Blume's Orama search: the same
//! BM25 parameters, field boosts, prefix matching, per-locale tokenizers and
//! strict bigram pass. Over the sigiro docs it ranks exactly as Blume does
//! (`answers_match_blume` replays recorded Blume answers). One part is an
//! approximation: Blume segments non-Latin text with ICU's `Intl.Segmenter`,
//! modeled here by script boundaries. That is exact for the Korean, Japanese
//! and Chinese docs, whose CJK runs are re-cut into bigrams either way, but a
//! language ICU segments by dictionary without bigrams (Thai) would differ.
//!
//! Plain BM25 over a ~1 MB corpus needs no search engine, so there is no
//! tantivy: the index is built in memory when `sigiro mcp` starts, which then
//! answers its first request in about 0.2 s.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::sync::{Arc, OnceLock};

use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

#[derive(RustEmbed)]
#[folder = "www/dist-mcp/"]
#[allow_missing = true]
struct Embedded;

/// The snapshot file inside `www/dist-mcp` (gzip-compressed JSON).
pub const SNAPSHOT_FILE: &str = "mcp-data.json.gz";

/// `search_docs` defaults, from Blume's `query.ts`.
const DEFAULT_SEARCH_LIMIT: usize = 8;
const MAX_SEARCH_LIMIT: usize = 20;
/// Excerpt length (UTF-16 code units, as JavaScript counts) for a page with no
/// description.
const EXCERPT_LENGTH: usize = 200;
/// Orama's BM25 defaults.
const BM25_K: f64 = 1.2;
const BM25_B: f64 = 0.75;
const BM25_D: f64 = 0.5;

/// Blume's `McpData`, as `export-mcp-data.ts` writes it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    base: String,
    default_locale: Option<String>,
    archived_versions: Option<Vec<String>>,
    documents: Vec<Document>,
    pages: HashMap<String, String>,
    routes: Vec<RouteEntry>,
    site: Option<String>,
    navigation_json: String,
    navigation_by_locale_json: Option<HashMap<String, String>>,
    navigation_by_version_json: Option<BTreeMap<String, BTreeMap<String, String>>>,
    tools: Vec<Tool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    content: String,
    #[serde(default)]
    description: String,
    route: String,
    title: String,
    locale: Option<String>,
    version: Option<String>,
    content_type: Option<String>,
    facets: Option<BTreeMap<String, String>>,
    boost: Option<f64>,
    #[serde(default)]
    keywords: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteEntry {
    content_type: String,
    description: Option<String>,
    facets: Option<BTreeMap<String, String>>,
    last_modified: Option<String>,
    locale: String,
    route: String,
    title: String,
    version: String,
}

/// One `search_docs` hit. Field order is Blume's, so the JSON text matches.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchHit<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<&'a str>,
    excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    facets: Option<&'a BTreeMap<String, String>>,
    route: &'a str,
    title: &'a str,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
}

/// One `list_pages` entry. Field order is Blume's.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PageListing<'a> {
    content_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    facets: Option<&'a BTreeMap<String, String>>,
    last_modified: Option<&'a str>,
    route: &'a str,
    title: &'a str,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
}

/// The docs MCP over one snapshot.
pub struct OfflineDocs {
    data: Snapshot,
    index: SearchIndex,
}

impl std::fmt::Debug for OfflineDocs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfflineDocs")
            .field("documents", &self.data.documents.len())
            .field("pages", &self.data.pages.len())
            .finish()
    }
}

impl OfflineDocs {
    /// Parse a snapshot (plain JSON).
    pub fn from_json(bytes: &[u8]) -> anyhow::Result<Self> {
        let data: Snapshot = serde_json::from_slice(bytes)?;
        anyhow::ensure!(
            !data.tools.is_empty(),
            "documentation snapshot advertises no tools"
        );
        let index = SearchIndex::build(&data.documents, data.default_locale.as_deref());
        Ok(Self { data, index })
    }

    /// Parse a gzip-compressed snapshot, as the binary embeds it.
    pub fn from_gzip(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut json = Vec::new();
        flate2::read::GzDecoder::new(bytes).read_to_end(&mut json)?;
        Self::from_json(&json)
    }

    /// The snapshot built into this binary, parsed once. `None` when the
    /// binary was built without the docs export.
    pub fn embedded() -> Option<Arc<Self>> {
        static EMBEDDED: OnceLock<Option<Arc<OfflineDocs>>> = OnceLock::new();
        EMBEDDED
            .get_or_init(|| {
                let file = Embedded::get(SNAPSHOT_FILE)?;
                match Self::from_gzip(&file.data) {
                    Ok(docs) => Some(Arc::new(docs)),
                    Err(error) => {
                        tracing::warn!("built-in documentation snapshot is unreadable: {error:#}");
                        None
                    }
                }
            })
            .clone()
    }

    /// The tool definitions, exactly as the hosted `tools/list` advertises them.
    pub fn tools(&self) -> Vec<Tool> {
        self.data.tools.clone()
    }

    /// Whether `name` is one of these tools.
    pub fn has_tool(&self, name: &str) -> bool {
        self.data.tools.iter().any(|tool| tool.name == name)
    }

    /// Answer one tool call. Unknown tools are a tool error, not a panic.
    pub fn call(&self, name: &str, arguments: &JsonObject) -> CallToolResult {
        match name {
            "search_docs" => text(pretty(&self.search_docs(arguments))),
            "get_page" => {
                let key = self.normalize_route(&text_arg(arguments.get("route")));
                match self.data.pages.get(&key) {
                    Some(markdown) => text(markdown.clone()),
                    None => error(format!(
                        "No page found at \"{key}\". Use list_pages or search_docs to find valid routes."
                    )),
                }
            }
            "list_pages" => text(pretty(&self.list_pages(arguments))),
            "get_navigation" => match self.navigation(arguments) {
                Ok(json) => text(json.to_string()),
                Err(message) => error(message),
            },
            _ => error(format!("Unknown tool: {name}")),
        }
    }

    /// Blume's `normalizeRoute`: a route, a `.md` URL or a full page URL, to
    /// the `pages` key.
    fn normalize_route(&self, input: &str) -> String {
        let mut value = input.trim().to_string();
        let lower = value.to_ascii_lowercase();
        if (lower.starts_with("http://") || lower.starts_with("https://"))
            && let Ok(url) = url::Url::parse(&value)
        {
            value = url.path().to_string();
        }
        if let Some(decoded) = decode_uri(&value) {
            value = decoded;
        }
        let trimmed = value.trim_end_matches('/');
        let no_suffix = trimmed
            .strip_suffix(".mdx")
            .or_else(|| trimmed.strip_suffix(".md"))
            .unwrap_or(trimmed);
        strip_base_path(&self.data.base, &normalize_page_route(no_suffix))
    }

    /// Blume's `urlFor`.
    fn url_for(&self, route: &str) -> String {
        let path = mount_base_path(&self.data.base, route);
        match &self.data.site {
            Some(site) => format!("{}{path}", site.trim_end_matches('/')),
            None => path,
        }
    }

    /// Blume's `asVersionScope`: `None` disables the filter.
    fn version_scope(&self, value: Option<&str>) -> Option<String> {
        self.data.archived_versions.as_ref()?;
        match value {
            Some("all") => None,
            None | Some("latest" | "current") => Some(String::new()),
            Some(id) => Some(id.to_string()),
        }
    }

    fn search_docs(&self, arguments: &JsonObject) -> Vec<SearchHit<'_>> {
        let query = text_arg(arguments.get("query"));
        let limit = limit_arg(arguments.get("limit")).unwrap_or(DEFAULT_SEARCH_LIMIT);
        let filters = Filters {
            content_types: content_types_arg(arguments.get("contentTypes")),
            facets: filters_arg(arguments.get("filters")),
            locale: optional_text_arg(arguments.get("locale")),
            version: self.version_scope(optional_text_arg(arguments.get("version")).as_deref()),
        };
        let versioned = self.data.archived_versions.is_some();
        self.index
            .query(&self.data.documents, &query, limit, &filters)
            .into_iter()
            .map(|i| {
                let doc = &self.data.documents[i];
                SearchHit {
                    content_type: doc.content_type.as_deref(),
                    excerpt: excerpt(doc),
                    facets: doc.facets.as_ref(),
                    route: &doc.route,
                    title: &doc.title,
                    url: self.url_for(&doc.route),
                    version: versioned.then(|| doc.version.as_deref().unwrap_or("")),
                }
            })
            .collect()
    }

    fn list_pages(&self, arguments: &JsonObject) -> Vec<PageListing<'_>> {
        let content_types = content_types_arg(arguments.get("contentTypes"));
        let facets = filters_arg(arguments.get("filters"));
        let locale = optional_text_arg(arguments.get("locale"));
        let scope = self.version_scope(optional_text_arg(arguments.get("version")).as_deref());
        let versioned = self.data.archived_versions.is_some();
        self.data
            .routes
            .iter()
            .filter(|route| {
                content_types
                    .as_ref()
                    .is_none_or(|types| types.contains(&route.content_type))
                    && facets
                        .as_ref()
                        .is_none_or(|wanted| matches_facets(route.facets.as_ref(), wanted))
                    && locale.as_ref().is_none_or(|l| &route.locale == l)
                    && scope.as_ref().is_none_or(|v| &route.version == v)
            })
            .map(|route| PageListing {
                content_type: &route.content_type,
                description: route.description.as_deref(),
                facets: route.facets.as_ref(),
                last_modified: route.last_modified.as_deref(),
                route: &route.route,
                title: &route.title,
                url: self.url_for(&route.route),
                version: versioned.then_some(route.version.as_str()),
            })
            .collect()
    }

    /// Blume's `getNavigation`, returning the tree's JSON text.
    fn navigation(&self, arguments: &JsonObject) -> Result<&str, String> {
        let locale = optional_text_arg(arguments.get("locale"));
        let version = optional_text_arg(arguments.get("version"));
        if let (Some(id), Some(archived)) = (&version, &self.data.archived_versions)
            && !archived.contains(id)
        {
            return Err(format!(
                "Unknown version \"{id}\". Archived versions: {}.",
                archived.join(", ")
            ));
        }
        let by_version = version
            .as_ref()
            .and_then(|id| self.data.navigation_by_version_json.as_ref()?.get(id));
        if let Some(by_locale) = by_version {
            let default = self.data.default_locale.clone().unwrap_or_default();
            return Ok(locale
                .as_ref()
                .and_then(|l| by_locale.get(l))
                .or_else(|| by_locale.get(&default))
                .or_else(|| by_locale.values().next())
                .unwrap_or(&self.data.navigation_json));
        }
        Ok(locale
            .as_ref()
            .and_then(|l| self.data.navigation_by_locale_json.as_ref()?.get(l))
            .unwrap_or(&self.data.navigation_json))
    }
}

/// A successful answer. Blume leaves `isError` out rather than sending
/// `false`, so this does too.
fn text(value: String) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(value)]);
    result.is_error = None;
    result
}

fn error(value: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(value)])
}

/// `JSON.stringify(value, null, 2)`.
fn pretty<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "[]".to_string())
}

// ---- Argument coercion, after Blume's Zod preprocess pipes: an argument is
// never rejected for its shape, only read as "absent" when unusable.

fn text_arg(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn optional_text_arg(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn content_types_arg(value: Option<&Value>) -> Option<Vec<String>> {
    let list: Vec<String> = match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    };
    (!list.is_empty()).then_some(list)
}

fn filters_arg(value: Option<&Value>) -> Option<BTreeMap<String, String>> {
    let Some(Value::Object(map)) = value else {
        return None;
    };
    let entries: BTreeMap<String, String> = map
        .iter()
        .filter_map(|(key, value)| {
            let value = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => js_number_string(n),
                Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some((key.clone(), value))
        })
        .collect();
    (!entries.is_empty()).then_some(entries)
}

/// `String(n)` for the integers a facet filter realistically carries.
fn js_number_string(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
        _ => n.to_string(),
    }
}

/// JavaScript's `Number(value)` for JSON values; `None` where it gives `NaN`.
fn js_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if s.trim().is_empty() => Some(0.0),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Null => Some(0.0),
        // An array converts through its string form: `[]` is "", `[7]` is "7".
        Value::Array(items) => match items.as_slice() {
            [] => Some(0.0),
            [Value::Bool(_) | Value::Object(_)] => None,
            [item] => js_number(item),
            _ => None,
        },
        Value::Object(_) => None,
    }
}

/// `Number(value)`, truncated and clamped to `1..=20`; non-numeric is "absent".
fn limit_arg(value: Option<&Value>) -> Option<usize> {
    let number = js_number(value?)?;
    if !number.is_finite() {
        return None;
    }
    let clamped = number.trunc().clamp(1.0, MAX_SEARCH_LIMIT as f64);
    // In 1..=20 by the clamp above.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(clamped as usize)
}

fn matches_facets(
    facets: Option<&BTreeMap<String, String>>,
    wanted: &BTreeMap<String, String>,
) -> bool {
    wanted
        .iter()
        .all(|(key, value)| facets.and_then(|f| f.get(key)) == Some(value))
}

// ---- Routes, after Blume's `core/base-path.ts`.

fn normalize_page_route(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn mount_base_path(base: &str, route: &str) -> String {
    if base.is_empty() {
        route.to_string()
    } else if route == "/" {
        base.to_string()
    } else {
        format!("{base}{route}")
    }
}

fn strip_base_path(base: &str, route: &str) -> String {
    if base.is_empty() {
        return route.to_string();
    }
    if route == base {
        return "/".to_string();
    }
    match route.strip_prefix(base) {
        Some(rest) if rest.starts_with('/') => rest.to_string(),
        _ => route.to_string(),
    }
}

/// JavaScript's `decodeURI`: percent-escapes are decoded except those of the
/// reserved characters it leaves alone. `None` on a malformed escape or
/// invalid UTF-8, where JavaScript throws (and Blume keeps the input).
fn decode_uri(input: &str) -> Option<String> {
    const RESERVED: &[u8] = b";/?:@&=+$,#";
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let byte = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            if RESERVED.contains(&byte) {
                out.extend_from_slice(&bytes[i..i + 3]);
            } else {
                out.push(byte);
            }
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Blume's `excerptFor`: the description, else the head of the content.
fn excerpt(doc: &Document) -> String {
    if !doc.description.is_empty() {
        return doc.description.clone();
    }
    let mut units = 0;
    let mut head = String::new();
    for c in doc.content.chars() {
        units += c.len_utf16();
        if units > EXCERPT_LENGTH {
            break;
        }
        head.push(c);
    }
    let head = head.trim();
    if doc.content.encode_utf16().count() > EXCERPT_LENGTH {
        format!("{head}…")
    } else {
        head.to_string()
    }
}

// ---- Search: Orama's BM25 over Blume's tokenization.

/// How a locale's text is cut into index terms (Blume's `orama-index.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tokenizer {
    /// Latin-script languages: Orama's default splitter, Latin folded.
    Latin,
    /// Non-Latin scripts written with spaces (Korean): one term per word.
    Words,
    /// Chinese and Japanese: CJK runs re-cut into character bigrams.
    Bigrams,
}

impl Tokenizer {
    fn for_locale(locale: &str) -> Self {
        let language = locale
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match language.as_str() {
            "zh" | "ja" | "yue" | "cmn" | "wuu" | "hak" | "nan" => Self::Bigrams,
            "ko" | "ru" | "uk" | "bg" | "sr" | "mk" | "be" | "el" | "he" | "ar" | "fa" | "hi"
            | "mr" | "ne" | "th" | "ka" | "hy" | "bn" | "ta" | "te" => Self::Words,
            _ => Self::Latin,
        }
    }

    fn tokenize(self, raw: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut push = |token: String| {
            if !token.is_empty() && !tokens.contains(&token) {
                tokens.push(token);
            }
        };
        match self {
            Self::Latin => {
                let folded = fold_latin(&raw.to_lowercase());
                for part in folded.split(|c: char| !is_latin_token_char(c)) {
                    push(part.to_string());
                }
            }
            Self::Words | Self::Bigrams => {
                let text: String = raw
                    .replace('\u{AD}', "")
                    .nfc()
                    .collect::<String>()
                    .to_lowercase();
                let bigram = self == Self::Bigrams;
                let mut run = String::new();
                let flush = |run: &mut String, push: &mut dyn FnMut(String)| {
                    if !run.is_empty() {
                        for gram in bigrams(run) {
                            push(gram);
                        }
                        run.clear();
                    }
                };
                for term in terms(&text) {
                    match term {
                        Term::Gap => flush(&mut run, &mut push),
                        Term::Word(word) if bigram && word.chars().all(is_bigram_char) => {
                            run.push_str(word);
                        }
                        Term::Word(word) => {
                            flush(&mut run, &mut push);
                            push(fold_latin(word));
                        }
                    }
                }
                flush(&mut run, &mut push);
            }
        }
        tokens
    }
}

/// A Latin-tokenizer term character: Latin letters, ASCII digits, `_`, `'`, `-`.
fn is_latin_token_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '_' | '\'' | '-') || is_latin_letter(c)
}

fn is_latin_letter(c: char) -> bool {
    static LATIN: OnceLock<regex::Regex> = OnceLock::new();
    if c.is_ascii() {
        return c.is_ascii_alphabetic();
    }
    let mut buf = [0; 4];
    LATIN
        .get_or_init(|| regex::Regex::new(r"^\p{Script=Latin}$").expect("valid regex"))
        .is_match(c.encode_utf8(&mut buf))
}

/// Characters of the scripts Blume bigrams: Han, Hiragana, Katakana and the
/// marks that only occur inside such words.
fn is_bigram_char(c: char) -> bool {
    static CJK: OnceLock<regex::Regex> = OnceLock::new();
    if c.is_ascii() {
        return false;
    }
    let mut buf = [0; 4];
    CJK.get_or_init(|| {
        regex::Regex::new(r"^[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}々〆〇ーﾞﾟ]$")
            .expect("valid regex")
    })
    .is_match(c.encode_utf8(&mut buf))
}

/// Characters ICU's word segmenter keeps apart from letters of other scripts
/// even inside one UAX #29 word: `sigiro를` and `5분` segment as two words.
fn is_hangul(c: char) -> bool {
    static HANGUL: OnceLock<regex::Regex> = OnceLock::new();
    if c.is_ascii() {
        return false;
    }
    let mut buf = [0; 4];
    HANGUL
        .get_or_init(|| regex::Regex::new(r"^\p{Script=Hangul}$").expect("valid regex"))
        .is_match(c.encode_utf8(&mut buf))
}

/// The segment class a word cannot span: Hangul, the bigrammed CJK scripts,
/// or everything else.
fn segment_class(c: char) -> u8 {
    if is_bigram_char(c) {
        2
    } else if is_hangul(c) {
        1
    } else {
        0
    }
}

fn bigrams(run: &str) -> Vec<String> {
    let chars: Vec<char> = run.chars().collect();
    if chars.len() == 1 {
        return vec![run.to_string()];
    }
    chars.windows(2).map(|w| w.iter().collect()).collect()
}

enum Term<'a> {
    Word(&'a str),
    Gap,
}

fn is_term_char(c: char) -> bool {
    c.is_alphanumeric() || unicode_normalization::char::is_combining_mark(c)
}

/// Blume's `TERM`: runs of letters, marks and digits, keeping an apostrophe
/// before a letter and a `.`/`,` between digits inside the run. A change of
/// [`segment_class`] also ends a word, where ICU's word segmenter starts a new
/// segment. Anything else is a gap, which ends a bigram run.
fn terms(text: &str) -> Vec<Term<'_>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let end_of = |i: usize| chars.get(i).map_or(text.len(), |(at, _)| *at);
    let mut i = 0;
    while i < chars.len() {
        let (_, c) = chars[i];
        let joins = start.is_some()
            && i + 1 < chars.len()
            && segment_class(chars[i - 1].1) == segment_class(chars[i + 1].1)
            && ((matches!(c, '\'' | '’') && chars[i + 1].1.is_alphabetic())
                || (matches!(c, '.' | ',')
                    && chars[i - 1].1.is_numeric()
                    && chars[i + 1].1.is_numeric()));
        if is_term_char(c) || joins {
            if let Some(s) = start {
                let prev = chars[i - 1].1;
                if is_term_char(prev) && segment_class(prev) != segment_class(c) && !joins {
                    out.push(Term::Word(&text[end_of(s)..end_of(i)]));
                    start = Some(i);
                }
            } else {
                start = Some(i);
            }
        } else {
            if let Some(s) = start.take() {
                out.push(Term::Word(&text[end_of(s)..end_of(i)]));
            }
            out.push(Term::Gap);
        }
        i += 1;
    }
    if let Some(s) = start {
        out.push(Term::Word(&text[end_of(s)..]));
    }
    out
}

/// Blume's `foldLatin` / `foldLatinTerm`: diacritics off Latin letters, a
/// few letters spelled out (`ß` → `ss`), compatibility forms folded. Text of
/// other scripts passes through untouched. Input is already lowercase.
fn fold_latin(lowered: &str) -> String {
    let mut out = String::with_capacity(lowered.len());
    let mut after_latin = false;
    for c in lowered.chars().filter(|c| *c != '\u{AD}') {
        if unicode_normalization::char::is_combining_mark(c) {
            if !after_latin {
                out.push(c);
            }
            continue;
        }
        after_latin = is_latin_letter(c);
        if !after_latin || c.is_ascii() {
            out.push(c);
            continue;
        }
        match c {
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            'ø' => out.push('o'),
            'ł' => out.push('l'),
            'đ' | 'ð' => out.push('d'),
            'ħ' => out.push('h'),
            'ı' => out.push('i'),
            'þ' => out.push_str("th"),
            _ => out.extend(
                std::iter::once(c)
                    .nfkd()
                    .filter(|m| !unicode_normalization::char::is_combining_mark(*m))
                    .flat_map(char::to_lowercase),
            ),
        }
    }
    out
}

/// The searched properties and their boosts (Blume's `BOOST`).
const FIELDS: [(Field, f64); 4] = [
    (Field::Title, 3.0),
    (Field::Keywords, 3.0),
    (Field::Description, 2.0),
    (Field::Content, 1.0),
];

#[derive(Debug, Clone, Copy)]
enum Field {
    Title,
    Keywords,
    Description,
    Content,
}

impl Field {
    fn index(self) -> usize {
        self as usize
    }
}

/// One property's inverted index, as Orama keeps it.
#[derive(Default)]
struct FieldIndex {
    /// term → documents containing it, in insertion order.
    postings: BTreeMap<String, Vec<usize>>,
    /// Per document: its token count and each token's frequency (count/len).
    lengths: Vec<usize>,
    frequencies: Vec<HashMap<String, f64>>,
    avg_length: f64,
}

struct SearchIndex {
    fields: [FieldIndex; 4],
    tokenizers: Vec<Tokenizer>,
    /// Each document's search group: `None` for pages searched with the Latin
    /// tokenizer together, else the locale whose own tokenizer searches them.
    groups: Vec<Option<String>>,
    /// Each document's `search.boost`.
    boosts: Vec<f64>,
}

#[derive(Default)]
struct Filters {
    content_types: Option<Vec<String>>,
    facets: Option<BTreeMap<String, String>>,
    locale: Option<String>,
    version: Option<String>,
}

impl SearchIndex {
    /// `default_locale` is the site's: a non-Latin default tokenizes every
    /// page with its tokenizer, a Latin one gives each non-Latin locale its own.
    fn build(documents: &[Document], default_locale: Option<&str>) -> Self {
        let mut fields: [FieldIndex; 4] = Default::default();
        let site = Tokenizer::for_locale(default_locale.unwrap_or_default());
        let tokenizers: Vec<Tokenizer> = documents
            .iter()
            .map(|doc| match site {
                Tokenizer::Latin => {
                    Tokenizer::for_locale(doc.locale.as_deref().unwrap_or_default())
                }
                other => other,
            })
            .collect();
        let groups = documents
            .iter()
            .zip(&tokenizers)
            .map(|(doc, tokenizer)| match (site, tokenizer) {
                (Tokenizer::Latin, Tokenizer::Words | Tokenizer::Bigrams) => doc.locale.clone(),
                _ => None,
            })
            .collect();
        for (id, doc) in documents.iter().enumerate() {
            let tokenizer = tokenizers[id];
            for (field, _) in FIELDS {
                let tokens: Vec<String> = match field {
                    Field::Title => tokenizer.tokenize(&doc.title),
                    Field::Description => tokenizer.tokenize(&doc.description),
                    Field::Content => tokenizer.tokenize(&doc.content),
                    // An array property: each element is inserted on its own,
                    // and the last one's length is the one Orama keeps.
                    Field::Keywords => doc
                        .keywords
                        .last()
                        .map(|k| tokenizer.tokenize(k))
                        .unwrap_or_default(),
                };
                let index = &mut fields[field.index()];
                let len = tokens.len();
                let mut frequencies = HashMap::new();
                for token in &tokens {
                    // Tokens are unique per value, so every count is 1.
                    #[allow(clippy::cast_precision_loss)]
                    frequencies.insert(token.clone(), 1.0 / len as f64);
                    index.postings.entry(token.clone()).or_default().push(id);
                }
                if field.index() == Field::Keywords.index() {
                    for keyword in doc.keywords.iter().rev().skip(1) {
                        for token in tokenizer.tokenize(keyword) {
                            let postings = index.postings.entry(token).or_default();
                            if postings.last() != Some(&id) {
                                postings.push(id);
                            }
                        }
                    }
                }
                index.lengths.push(len);
                index.frequencies.push(frequencies);
            }
        }
        for index in &mut fields {
            #[allow(clippy::cast_precision_loss)]
            let avg =
                index.lengths.iter().sum::<usize>() as f64 / index.lengths.len().max(1) as f64;
            index.avg_length = avg;
        }
        Self {
            fields,
            tokenizers,
            groups,
            boosts: documents
                .iter()
                .map(|doc| doc.boost.unwrap_or(1.0))
                .collect(),
        }
    }

    /// Blume's `queryOramaIndex`: per tokenizer group (a non-Latin locale's
    /// pages are searched with that locale's tokenizer), Orama's search over
    /// the shared statistics, merged by score and cut to `limit`.
    fn query(
        &self,
        documents: &[Document],
        term: &str,
        limit: usize,
        filters: &Filters,
    ) -> Vec<usize> {
        let allowed: Vec<bool> = documents
            .iter()
            .map(|doc| {
                filters
                    .locale
                    .as_ref()
                    .is_none_or(|l| doc.locale.as_ref() == Some(l))
                    && filters
                        .version
                        .as_ref()
                        .is_none_or(|v| doc.version.as_deref().unwrap_or("") == v)
                    && filters.content_types.as_ref().is_none_or(|types| {
                        doc.content_type.as_ref().is_some_and(|t| types.contains(t))
                    })
                    && filters
                        .facets
                        .as_ref()
                        .is_none_or(|wanted| matches_facets(doc.facets.as_ref(), wanted))
            })
            .collect();
        // The Latin group first, then each translated locale, as Blume merges.
        let mut groups: Vec<(Option<String>, Tokenizer)> = Vec::new();
        let latin = self.groups.iter().position(Option::is_none);
        for id in latin.into_iter().chain(0..self.groups.len()) {
            let key = (self.groups[id].clone(), self.tokenizers[id]);
            if !groups.contains(&key) {
                groups.push(key);
            }
        }
        let mut hits: Vec<(usize, f64)> = Vec::new();
        for (group, tokenizer) in groups {
            let in_group: Vec<bool> = allowed
                .iter()
                .zip(&self.groups)
                .map(|(ok, g)| *ok && *g == group)
                .collect();
            if !in_group.contains(&true) {
                continue;
            }
            let strict = (tokenizer == Tokenizer::Bigrams)
                .then(|| self.search(term, tokenizer, &in_group, true))
                .filter(|found| !found.is_empty());
            let mut found =
                strict.unwrap_or_else(|| self.search(term, tokenizer, &in_group, false));
            found.truncate(limit);
            hits.extend(found);
        }
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits.truncate(limit);
        hits.into_iter().map(|(id, _)| id).collect()
    }

    /// Orama's full-text `search` over the documents `allowed` admits. `strict`
    /// is threshold 0: only documents with every query token in one property.
    fn search(
        &self,
        term: &str,
        tokenizer: Tokenizer,
        allowed: &[bool],
        strict: bool,
    ) -> Vec<(usize, f64)> {
        let mut tokens = tokenizer.tokenize(term);
        if tokens.is_empty() {
            if !term.is_empty() {
                return Vec::new();
            }
            tokens.push(String::new());
        }
        let keywords = tokens.len();
        let docs = self.tokenizers.len();
        let mut scores: HashMap<usize, f64> = HashMap::new();
        let mut matches: HashMap<usize, [usize; 4]> = HashMap::new();
        let mut token_found = vec![false; keywords];
        for (field, boost) in FIELDS {
            let index = &self.fields[field.index()];
            for (t, token) in tokens.iter().enumerate() {
                for (word, ids) in index
                    .postings
                    .range::<str, _>((
                        std::ops::Bound::Included(token.as_str()),
                        std::ops::Bound::Unbounded,
                    ))
                    .take_while(|(word, _)| word.starts_with(token.as_str()))
                {
                    token_found[t] = true;
                    #[allow(clippy::cast_precision_loss)]
                    let occurrences = ids.len() as f64;
                    #[allow(clippy::cast_precision_loss)]
                    let idf = (1.0 + (docs as f64 - occurrences + 0.5) / (occurrences + 0.5)).ln();
                    for &id in ids {
                        if !allowed[id] {
                            continue;
                        }
                        matches.entry(id).or_default()[field.index()] += 1;
                        let tf = index.frequencies[id].get(word).copied().unwrap_or(0.0);
                        #[allow(clippy::cast_precision_loss)]
                        let length = index.lengths[id] as f64;
                        let bm25 = idf * (BM25_D + tf * (BM25_K + 1.0))
                            / (tf + BM25_K * (1.0 - BM25_B + BM25_B * length / index.avg_length));
                        *scores.entry(id).or_default() += bm25 * boost;
                    }
                }
            }
        }
        // Ranked by score times the page's boost; ties go to the earlier page.
        let mut results: Vec<(usize, f64)> = scores
            .into_iter()
            .map(|(id, score)| (id, score * self.boosts[id]))
            .collect();
        results.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        if strict && keywords > 1 {
            if token_found.iter().any(|found| !found) {
                return Vec::new();
            }
            results.retain(|(id, _)| matches[id].contains(&keywords));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> OfflineDocs {
        OfflineDocs::from_json(include_bytes!("../tests/fixtures/docs-mcp/mcp-data.json"))
            .expect("fixture snapshot parses")
    }

    fn call(docs: &OfflineDocs, name: &str, args: &Value) -> (bool, String) {
        let args = args.as_object().cloned().unwrap_or_default();
        let result = docs.call(name, &args);
        let text = result.content[0].as_text().expect("text").text.clone();
        (result.is_error == Some(true), text)
    }

    /// A binary built by `mise run build` carries the real snapshot; check
    /// it parses whenever this build has one.
    #[test]
    fn embedded_snapshot_parses_when_built_in() {
        if let Some(file) = Embedded::get(SNAPSHOT_FILE) {
            let docs = OfflineDocs::from_gzip(&file.data).expect("embedded snapshot parses");
            assert_eq!(docs.tools().len(), 4);
            assert!(OfflineDocs::embedded().is_some());
        }
    }

    #[test]
    fn tools_are_the_hosted_four() {
        let names: Vec<String> = fixture()
            .tools()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(
            names,
            ["search_docs", "get_page", "list_pages", "get_navigation"]
        );
    }

    #[test]
    fn get_page_accepts_routes_markdown_urls_and_full_urls() {
        let docs = fixture();
        for route in [
            "/docs/how-to/executor",
            "/docs/how-to/executor/",
            "docs/how-to/executor.md",
            "https://sigiro.com/docs/how-to/executor",
            "  https://SIGIRO.com/docs/how-to/executor.mdx ",
        ] {
            let (is_error, text) = call(&docs, "get_page", &json!({ "route": route }));
            assert!(!is_error, "{route}");
            assert!(text.contains("Executor"), "{route}");
        }
        let (is_error, text) = call(&docs, "get_page", &json!({ "route": "/nope" }));
        assert!(is_error);
        assert_eq!(
            text,
            "No page found at \"/nope\". Use list_pages or search_docs to find valid routes."
        );
    }

    #[test]
    fn list_pages_filters_by_locale_and_type_and_keeps_blumes_shape() {
        let docs = fixture();
        let (_, all) = call(&docs, "list_pages", &json!({}));
        let all: Vec<Value> = serde_json::from_str(&all).expect("json");
        let (_, ja) = call(&docs, "list_pages", &json!({ "locale": " ja " }));
        let ja: Vec<Value> = serde_json::from_str(&ja).expect("json");
        assert!(!ja.is_empty() && ja.len() < all.len());
        assert!(
            ja.iter()
                .all(|p| p["route"].as_str().unwrap().starts_with("/docs/ja"))
        );
        let keys: Vec<&str> = ja[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "contentType",
                "description",
                "lastModified",
                "route",
                "title",
                "url"
            ]
        );
        assert!(ja[0]["lastModified"].is_null());
        assert!(
            ja[0]["url"]
                .as_str()
                .unwrap()
                .starts_with("https://sigiro.com/docs/ja")
        );
        let (_, none) = call(&docs, "list_pages", &json!({ "contentTypes": "rfc" }));
        assert_eq!(none, "[]");
        let (_, same) = call(
            &docs,
            "list_pages",
            &json!({ "contentTypes": [], "filters": {} }),
        );
        assert_eq!(
            serde_json::from_str::<Vec<Value>>(&same).unwrap().len(),
            all.len()
        );
    }

    #[test]
    fn get_navigation_returns_the_locale_tree_or_the_default() {
        let docs = fixture();
        let (_, en) = call(&docs, "get_navigation", &json!({}));
        let (_, ja) = call(&docs, "get_navigation", &json!({ "locale": "ja" }));
        let (_, xx) = call(&docs, "get_navigation", &json!({ "locale": "xx" }));
        assert!(en.starts_with("{\n  \"actions\""));
        assert_ne!(en, ja);
        assert_eq!(en, xx);
        // Unversioned site: a version id is ignored, as hosted.
        let (is_error, v) = call(&docs, "get_navigation", &json!({ "version": "v9" }));
        assert!(!is_error);
        assert_eq!(v, en);
    }

    #[test]
    fn search_ranks_the_page_about_the_term_first() {
        let docs = fixture();
        let (_, text) = call(
            &docs,
            "search_docs",
            &json!({ "query": "executor", "limit": 3 }),
        );
        let hits: Vec<Value> = serde_json::from_str(&text).expect("json");
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0]["route"], "/docs/how-to/executor");
        let keys: Vec<&str> = hits[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["contentType", "excerpt", "route", "title", "url"]);
        assert_eq!(hits[0]["url"], "https://sigiro.com/docs/how-to/executor");
    }

    #[test]
    fn search_prefix_matches_and_scopes_to_a_locale() {
        let docs = fixture();
        let (_, text) = call(
            &docs,
            "search_docs",
            &json!({ "query": "execu", "locale": "ja" }),
        );
        let hits: Vec<Value> = serde_json::from_str(&text).expect("json");
        assert!(!hits.is_empty());
        assert!(
            hits.iter()
                .all(|h| h["route"].as_str().unwrap().starts_with("/docs/ja"))
        );
    }

    #[test]
    fn search_bigrams_japanese_queries() {
        let docs = fixture();
        let (_, text) = call(
            &docs,
            "search_docs",
            &json!({ "query": "接続", "locale": "ja" }),
        );
        let hits: Vec<Value> = serde_json::from_str(&text).expect("json");
        assert_eq!(hits[0]["route"], "/docs/ja/how-to/executor");
    }

    #[test]
    fn search_limit_is_clamped_and_unmatched_queries_are_empty() {
        let docs = fixture();
        let (_, text) = call(
            &docs,
            "search_docs",
            &json!({ "query": "sigiro", "limit": 0 }),
        );
        assert_eq!(serde_json::from_str::<Vec<Value>>(&text).unwrap().len(), 1);
        let (_, text) = call(
            &docs,
            "search_docs",
            &json!({ "query": "sigiro", "limit": "999" }),
        );
        assert!(serde_json::from_str::<Vec<Value>>(&text).unwrap().len() <= MAX_SEARCH_LIMIT);
        let (_, text) = call(&docs, "search_docs", &json!({ "query": "zzzqqq" }));
        assert_eq!(text, "[]");
    }

    #[test]
    fn tokenizers_follow_blume() {
        assert_eq!(
            Tokenizer::Latin.tokenize("Prüfung: drill_down_sql, sigiro's service.version Ölß"),
            [
                "prufung",
                "drill_down_sql",
                "sigiro's",
                "service",
                "version",
                "olss"
            ]
        );
        assert_eq!(
            Tokenizer::Bigrams.tokenize("資金決済法とsigiro"),
            ["資金", "金決", "決済", "済法", "法と", "sigiro"]
        );
        assert_eq!(Tokenizer::Bigrams.tokenize("法"), ["法"]);
        assert_eq!(
            Tokenizer::Words.tokenize("sigiro를 연결 1.0.3 mode'가"),
            ["sigiro", "를", "연결", "1.0.3", "mode", "가"]
        );
    }

    #[test]
    fn excerpts_cut_at_200_utf16_units() {
        let doc = |content: &str| Document {
            content: content.to_string(),
            description: String::new(),
            route: "/".into(),
            title: String::new(),
            locale: None,
            version: None,
            content_type: None,
            facets: None,
            boost: None,
            keywords: Vec::new(),
        };
        assert_eq!(excerpt(&doc("short ")), "short");
        let long = "a".repeat(250);
        assert_eq!(excerpt(&doc(&long)), format!("{}…", "a".repeat(200)));
    }

    /// Every call in `calls.json` answers as Blume's own server did
    /// (`www/scripts/mcp-answers.ts` recorded `blume-answers.json`): the same
    /// text and error flag, and for `search_docs` the same ranked routes.
    #[test]
    fn answers_match_blume() {
        let docs = fixture();
        let calls: Vec<(String, Value)> =
            serde_json::from_str(include_str!("../tests/fixtures/docs-mcp/calls.json"))
                .expect("calls");
        let answers: Vec<Value> = serde_json::from_str(include_str!(
            "../tests/fixtures/docs-mcp/blume-answers.json"
        ))
        .expect("answers");
        assert_eq!(calls.len(), answers.len());
        let mut mismatches = Vec::new();
        for ((name, args), want) in calls.iter().zip(&answers) {
            let (is_error, text) = call(&docs, name, args);
            let got = match want.get("routes") {
                Some(_) => {
                    let hits: Vec<Value> = serde_json::from_str(&text).expect("search json");
                    let routes: Vec<&Value> = hits.iter().map(|hit| &hit["route"]).collect();
                    json!({ "isError": is_error, "routes": routes })
                }
                None => json!({ "isError": is_error, "text": text }),
            };
            if &got != want {
                mismatches.push(format!("{name} {args}"));
            }
        }
        assert!(
            mismatches.is_empty(),
            "answers differ from Blume's: {mismatches:#?}"
        );
    }

    #[test]
    fn decode_uri_keeps_reserved_escapes() {
        assert_eq!(decode_uri("/a%20b%2Fc").as_deref(), Some("/a b%2Fc"));
        assert_eq!(decode_uri("/%E6%97%A5").as_deref(), Some("/日"));
        assert_eq!(decode_uri("/%zz"), None);
    }
}
