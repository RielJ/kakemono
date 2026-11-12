//! Candidate image URLs from a fetched page: what kind of page it is, the image links on an HTML
//! page or in an RSS/Atom feed, and which known sites only work with JavaScript.
//!
//! Every function here is pure. Candidates are only guesses: the provider still reads each one's
//! size (`super::dimensions`) and keeps the wallpaper-sized ones.

use std::collections::HashSet;
use std::sync::LazyLock;

use percent_encoding::percent_decode_str;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use scraper::{ElementRef, Html, Selector};
use url::Url;

use super::MAX_CANDIDATES;

/// One image URL found on a page, before its size is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// The full-size image.
    pub url: Url,
    /// A smaller version for previews, when the page names one.
    pub preview: Option<Url>,
    /// Display name: the image's alt/title text, or its file name.
    pub title: String,
}

/// What a fetched link turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageKind {
    /// The link is an image itself.
    Image,
    /// An HTML page.
    Html,
    /// An RSS or Atom feed.
    Feed,
    /// Anything else (JSON, plain text, a PDF...).
    Other,
}

/// The feed is not well-formed XML, or not RSS/Atom at all.
#[derive(Debug, thiserror::Error)]
#[error("web: can't read this feed")]
pub(crate) struct FeedError {
    detail: String,
}

impl FeedError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// What the parser rejected, for logs (the Display text is the user-facing message).
    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

/// File extensions of the formats kakemono sets as wallpapers.
const IMAGE_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// Image formats that are never wallpapers (spacers, icons, logos). Skipping them up front keeps
/// the probe budget for real candidates; the size probe would reject them anyway.
const SKIPPED_EXTENSIONS: [&str; 4] = ["gif", "svg", "ico", "bmp"];

/// How many bytes of the body `kind_of` looks at for markup.
const SNIFF_BYTES: usize = 1024;

/// From the Content-Type header (may be empty) and the first bytes of the body.
///
/// A specific Content-Type wins; an empty or generic binary one falls back to the body's magic
/// bytes and leading markup.
pub(crate) fn kind_of(content_type: &str, prefix: &[u8]) -> PageKind {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "" | "application/octet-stream" | "binary/octet-stream" => sniff(prefix),
        "text/html" | "application/xhtml+xml" => PageKind::Html,
        "application/rss+xml"
        | "application/atom+xml"
        | "application/rdf+xml"
        | "application/xml"
        | "text/xml" => PageKind::Feed,
        image if image.starts_with("image/") => PageKind::Image,
        _ => PageKind::Other,
    }
}

/// The kind of a body with no useful Content-Type.
fn sniff(prefix: &[u8]) -> PageKind {
    let is_webp = prefix.len() >= 12 && prefix.starts_with(b"RIFF") && &prefix[8..12] == b"WEBP";
    if prefix.starts_with(b"\xFF\xD8\xFF") || prefix.starts_with(b"\x89PNG\r\n\x1a\n") || is_webp {
        return PageKind::Image;
    }
    let head = prefix.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(prefix);
    let head = &head[..head.len().min(SNIFF_BYTES)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    if !text.trim_start().starts_with('<') {
        return PageKind::Other;
    }
    // The root element decides: whichever marker comes first. A feed's root precedes any HTML
    // its items carry.
    let first = |markers: &[&str]| markers.iter().filter_map(|m| text.find(m)).min();
    match (
        first(&["<rss", "<feed", "<rdf:rdf"]),
        first(&["<!doctype html", "<html"]),
    ) {
        (Some(feed), Some(html)) if html < feed => PageKind::Html,
        (Some(_), _) => PageKind::Feed,
        (None, Some(_)) => PageKind::Html,
        (None, None) => PageKind::Other,
    }
}

static BASE: LazyLock<Selector> = LazyLock::new(|| selector("base[href]"));
static IMAGE_LINKS: LazyLock<Selector> = LazyLock::new(|| selector("a[href]"));
static LINK_THUMB: LazyLock<Selector> = LazyLock::new(|| selector("img[src]"));
static SRCSETS: LazyLock<Selector> =
    LazyLock::new(|| selector("img[srcset], picture source[srcset], [data-srcset]"));
static OG_IMAGE: LazyLock<Selector> =
    LazyLock::new(|| selector(r#"meta[property="og:image"][content]"#));
static TWITTER_IMAGE: LazyLock<Selector> = LazyLock::new(|| {
    selector(r#"meta[name="twitter:image"][content], meta[property="twitter:image"][content]"#)
});
static IMG_SRC: LazyLock<Selector> = LazyLock::new(|| selector("img[src]"));
static IMG_DATA_SRC: LazyLock<Selector> = LazyLock::new(|| selector("img[data-src]"));
static PICTURE_IMG: LazyLock<Selector> = LazyLock::new(|| selector("img"));

/// Parses a constant selector. Every constant is covered by `selectors_parse`, so this cannot
/// fail at runtime.
#[allow(clippy::expect_used)]
fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("constant CSS selector is valid")
}

/// Image candidates on an HTML page, best guesses first, at most [`MAX_CANDIDATES`].
///
/// Order: links to image files, the largest entry of each `srcset`, `og:image`, `twitter:image`,
/// then plain `img` sources. A thumbnail inside a link to an image file becomes that link's
/// preview, and an `img` with a `srcset` gives only its largest size, not its `src` too.
pub(crate) fn html_candidates(html: &str, page: &Url) -> Vec<Candidate> {
    let doc = Html::parse_document(html);
    let base = doc
        .select(&BASE)
        .next()
        .and_then(|b| b.attr("href"))
        .and_then(|href| page.join(href.trim()).ok())
        .unwrap_or_else(|| page.clone());
    let mut found = Collector::default();
    // Elements already turned into a candidate or a preview: an `img` inside an image link, or
    // one whose `srcset` gave its largest size. Their `src` is a smaller copy, not a new image.
    let mut used = HashSet::new();

    for link in doc.select(&IMAGE_LINKS) {
        let Some(url) = link.attr("href").and_then(|h| http_url(&base, h)) else {
            continue;
        };
        if !has_extension(&url, &IMAGE_EXTENSIONS) {
            continue;
        }
        let thumb = link.select(&LINK_THUMB).next();
        if let Some(img) = thumb {
            used.insert(img.id());
        }
        let preview = thumb
            .and_then(|img| img.attr("src"))
            .and_then(|s| http_url(&base, s));
        let label = label_of(link).or_else(|| thumb.and_then(label_of));
        found.push(url, preview, label);
    }

    for el in doc.select(&SRCSETS) {
        if !used.insert(el.id()) {
            continue;
        }
        let mut entries: Vec<SrcsetEntry> = ["srcset", "data-srcset"]
            .iter()
            .filter_map(|attr| el.attr(attr))
            .flat_map(parse_srcset)
            .filter_map(|(raw, size)| http_url(&base, &raw).map(|url| SrcsetEntry { url, size }))
            .collect();
        entries.sort_by_key(|e| e.size);
        let (Some(smallest), Some(largest)) = (entries.first(), entries.last()) else {
            continue;
        };
        let preview = (smallest.url != largest.url).then(|| smallest.url.clone());
        found.push(largest.url.clone(), preview, srcset_label(el));
    }

    for meta in [&*OG_IMAGE, &*TWITTER_IMAGE] {
        for el in doc.select(meta) {
            if let Some(url) = el.attr("content").and_then(|c| http_url(&base, c)) {
                found.push(url, None, None);
            }
        }
    }

    for (sel, attr) in [(&*IMG_SRC, "src"), (&*IMG_DATA_SRC, "data-src")] {
        for img in doc.select(sel) {
            if used.contains(&img.id()) {
                continue;
            }
            if let Some(url) = img.attr(attr).and_then(|s| http_url(&base, s)) {
                found.push(url, None, label_of(img));
            }
        }
    }

    found.into_vec()
}

/// One `srcset` entry that resolved to an http(s) URL.
struct SrcsetEntry {
    url: Url,
    size: SrcsetSize,
}

/// A `srcset` descriptor, ordered so the largest image sorts last: width descriptors (`800w`)
/// outrank density ones (`2x`), which are compared in thousandths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SrcsetSize {
    Density(u32),
    Width(u32),
}

/// The (URL, size) entries of a `srcset` attribute. URLs may contain commas
/// (`/w_400,h_300/a.jpg 400w`): an entry's URL runs to the next whitespace, and its descriptor to
/// the next comma.
fn parse_srcset(srcset: &str) -> Vec<(String, SrcsetSize)> {
    let mut entries = Vec::new();
    let mut rest = srcset;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.is_empty() {
            return entries;
        }
        let url_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (raw_url, after) = rest.split_at(url_end);
        let (url, descriptor) = if let Some(url) = raw_url.strip_suffix(',') {
            // "a.jpg, b.jpg 2x": a URL ending in a comma has no descriptor.
            rest = after;
            (url.trim_end_matches(','), "")
        } else {
            let desc_end = after.find(',').unwrap_or(after.len());
            let (descriptor, next) = after.split_at(desc_end);
            rest = next;
            (raw_url, descriptor.trim())
        };
        entries.push((url.to_owned(), descriptor_size(descriptor)));
    }
}

/// The size a `srcset` descriptor names; no descriptor or an unreadable one means `1x`.
fn descriptor_size(descriptor: &str) -> SrcsetSize {
    let descriptor = descriptor.to_ascii_lowercase();
    if let Some(width) = descriptor.strip_suffix('w').and_then(|w| w.parse().ok()) {
        return SrcsetSize::Width(width);
    }
    let density = descriptor
        .strip_suffix('x')
        .and_then(|x| x.parse::<f32>().ok())
        .filter(|x| x.is_finite() && *x > 0.0)
        .unwrap_or(1.0);
    // Rounded and clamped by the float-to-int cast; only the order matters.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive finite density, saturating cast is the intent"
    )]
    SrcsetSize::Density((density * 1000.0).round() as u32)
}

/// Alt or title text of an element, when it has any.
fn label_of(el: ElementRef<'_>) -> Option<String> {
    ["alt", "title"]
        .iter()
        .filter_map(|attr| el.attr(attr))
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_owned)
}

/// A `srcset` element's label; a `<picture>`'s `<source>` borrows its sibling `img`'s alt text.
fn srcset_label(el: ElementRef<'_>) -> Option<String> {
    label_of(el).or_else(|| {
        let picture = el.parent().and_then(ElementRef::wrap)?;
        (picture.value().name() == "picture")
            .then(|| picture.select(&PICTURE_IMG).next().and_then(label_of))
            .flatten()
    })
}

/// `raw` resolved against `base` when it is an http(s) URL, without its fragment.
fn http_url(base: &Url, raw: &str) -> Option<Url> {
    let mut url = base.join(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

/// The URL's last path segment, percent-decoded ("" for a directory URL).
pub(crate) fn file_name(url: &Url) -> String {
    let last = url.path().rsplit('/').next().unwrap_or_default();
    percent_decode_str(last).decode_utf8_lossy().into_owned()
}

/// Whether the URL's file name ends in one of `extensions` (lowercase, no dot), ignoring case.
fn has_extension(url: &Url, extensions: &[&str]) -> bool {
    let name = file_name(url);
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| extensions.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// Candidates in the order found: deduplicated, known non-wallpaper formats skipped, capped at
/// [`MAX_CANDIDATES`].
#[derive(Default)]
struct Collector {
    seen: HashSet<Url>,
    found: Vec<Candidate>,
}

impl Collector {
    fn push(&mut self, url: Url, preview: Option<Url>, label: Option<String>) {
        if self.found.len() >= MAX_CANDIDATES
            || has_extension(&url, &SKIPPED_EXTENSIONS)
            || !self.seen.insert(url.clone())
        {
            return;
        }
        let title = label.unwrap_or_else(|| match file_name(&url) {
            name if name.is_empty() => url.host_str().unwrap_or_default().to_owned(),
            name => name,
        });
        self.found.push(Candidate {
            url,
            preview,
            title,
        });
    }

    fn into_vec(self) -> Vec<Candidate> {
        self.found
    }
}

/// Image candidates in an RSS or Atom feed, at most [`MAX_CANDIDATES`].
///
/// Per item: image enclosures (RSS `enclosure`, Atom `link rel="enclosure"`), image
/// `media:content` (with `media:thumbnail` as the preview, never a candidate), then images in the
/// item's HTML (`description`, `content:encoded`, Atom `content`/`summary`).
pub(crate) fn feed_candidates(xml: &str, page: &Url) -> Result<Vec<Candidate>, FeedError> {
    let mut reader = Reader::from_str(xml);
    let mut parser = FeedParser::new(page);
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => parser.start(&tag, false)?,
            Ok(Event::Empty(tag)) => parser.start(&tag, true)?,
            Ok(Event::End(tag)) => parser.end(tag.local_name().as_ref()),
            Ok(Event::Text(text)) => parser.text(&text.xml10_content()),
            Ok(Event::CData(text)) => parser.text(&text.xml10_content()),
            Ok(Event::GeneralRef(entity)) => {
                let resolved = match entity.resolve_char_ref() {
                    Ok(Some(ch)) => ch.to_string(),
                    Ok(None) => quick_xml::escape::resolve_predefined_entity(&entity)
                        .map_or_else(|| format!("&{};", &*entity), str::to_owned),
                    Err(err) => return Err(FeedError::new(err.to_string())),
                };
                parser.text(&resolved);
            }
            Ok(Event::Eof) => break,
            Ok(Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_)) => {}
            Err(err) => return Err(FeedError::new(err.to_string())),
        }
    }
    parser.finish()
}

/// What the text of the element being read is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    Title,
    Html,
}

/// One RSS `item` / Atom `entry` being read.
#[derive(Default)]
struct Item {
    /// Depth of the item element in the open-element stack.
    depth: usize,
    title: Option<String>,
    /// Feed-level preview for every media candidate without its own.
    thumbnail: Option<Url>,
    media: Vec<(Url, Option<Url>)>,
    /// Index in `media` of the open `media:content`, whose child thumbnail is its own preview.
    open_media: Option<usize>,
    html: Vec<String>,
    /// Text being collected: what for, the element's depth, and the text so far.
    capture: Option<(Capture, usize, String)>,
}

struct FeedParser<'a> {
    page: &'a Url,
    /// Local names of the open elements.
    stack: Vec<String>,
    saw_root: bool,
    item: Option<Item>,
    found: Collector,
}

impl<'a> FeedParser<'a> {
    fn new(page: &'a Url) -> Self {
        Self {
            page,
            stack: Vec::new(),
            saw_root: false,
            item: None,
            found: Collector::default(),
        }
    }

    fn start(&mut self, tag: &BytesStart<'_>, empty: bool) -> Result<(), FeedError> {
        let name = tag.local_name().as_ref().to_owned();
        if !self.saw_root {
            if !matches!(name.as_str(), "rss" | "feed" | "RDF") {
                return Err(FeedError::new(format!(
                    "root element <{name}> is not a feed"
                )));
            }
            self.saw_root = true;
        }
        let depth = self.stack.len();
        self.element(tag, &name, depth, empty);
        if empty {
            // An empty element opens and closes at once; close what it opened.
            self.stack.push(name.clone());
            self.end(&name);
        } else {
            self.stack.push(name);
        }
        Ok(())
    }

    /// Reacts to an opening tag at `depth` (before it is pushed).
    fn element(&mut self, tag: &BytesStart<'_>, name: &str, depth: usize, empty: bool) {
        if matches!(name, "item" | "entry") && self.item.is_none() {
            self.item = Some(Item {
                depth,
                ..Item::default()
            });
            return;
        }
        let page = self.page;
        let Some(item) = self.item.as_mut() else {
            return;
        };
        if let Some((Capture::Html, _, text)) = item.capture.as_mut() {
            // Atom `type="xhtml"` content is inline markup: keep it as HTML text.
            text.push('<');
            text.push_str(tag);
            text.push_str(if empty { "/>" } else { ">" });
            return;
        }
        let attr = |key: &str| attribute(tag, key);
        let is_image = |url: &Url, kind: Option<&str>| {
            kind.is_some_and(|k| k.trim().to_ascii_lowercase().starts_with("image/"))
                || has_extension(url, &IMAGE_EXTENSIONS)
        };
        match name {
            "enclosure" => {
                if let Some(url) = attr("url").and_then(|u| http_url(page, &u))
                    && is_image(&url, attr("type").as_deref())
                {
                    item.media.push((url, None));
                }
            }
            "link" if attr("rel").as_deref() == Some("enclosure") => {
                if let Some(url) = attr("href").and_then(|u| http_url(page, &u))
                    && is_image(&url, attr("type").as_deref())
                {
                    item.media.push((url, None));
                }
            }
            "content" if attr("url").is_some() => {
                let Some(url) = attr("url").and_then(|u| http_url(page, &u)) else {
                    return;
                };
                let medium_image = attr("medium").as_deref() == Some("image");
                if medium_image || is_image(&url, attr("type").as_deref()) {
                    item.media.push((url, None));
                    if !empty {
                        item.open_media = Some(item.media.len() - 1);
                    }
                }
            }
            "thumbnail" => {
                let Some(url) = attr("url").and_then(|u| http_url(page, &u)) else {
                    return;
                };
                match item.open_media.and_then(|i| item.media.get_mut(i)) {
                    Some((_, preview)) => *preview = Some(url),
                    None => item.thumbnail = item.thumbnail.take().or(Some(url)),
                }
            }
            "title" if depth == item.depth + 1 => {
                item.capture = Some((Capture::Title, depth, String::new()));
            }
            "description" | "encoded" | "content" | "summary" if depth == item.depth + 1 => {
                item.capture = Some((Capture::Html, depth, String::new()));
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        self.stack.pop();
        let depth = self.stack.len();
        let Some(item) = self.item.as_mut() else {
            return;
        };
        if depth == item.depth {
            if let Some(item) = self.item.take() {
                self.flush(item);
            }
            return;
        }
        match item.capture.take() {
            Some((kind, at, text)) if at == depth => match kind {
                Capture::Title => {
                    item.title = Some(text.split_whitespace().collect::<Vec<_>>().join(" "));
                }
                Capture::Html => item.html.push(text),
            },
            Some((Capture::Html, at, mut text)) => {
                text.push_str("</");
                text.push_str(name);
                text.push('>');
                item.capture = Some((Capture::Html, at, text));
            }
            other => item.capture = other,
        }
        if name == "content" {
            item.open_media = None;
        }
    }

    fn text(&mut self, text: &str) {
        if let Some((_, _, buf)) = self.item.as_mut().and_then(|i| i.capture.as_mut()) {
            buf.push_str(text);
        }
    }

    fn flush(&mut self, item: Item) {
        let title = item.title.filter(|t| !t.is_empty());
        for (url, preview) in item.media {
            self.found.push(
                url,
                preview.or_else(|| item.thumbnail.clone()),
                title.clone(),
            );
        }
        for html in &item.html {
            for c in html_candidates(html, self.page) {
                self.found.push(c.url, c.preview, Some(c.title));
            }
        }
    }

    fn finish(self) -> Result<Vec<Candidate>, FeedError> {
        if !self.saw_root {
            return Err(FeedError::new("no root element"));
        }
        if let Some(open) = self.stack.last() {
            return Err(FeedError::new(format!("<{open}> is never closed")));
        }
        Ok(self.found.into_vec())
    }
}

/// An attribute's unescaped value; None when absent or malformed.
fn attribute(tag: &BytesStart<'_>, key: &str) -> Option<String> {
    let attr = tag
        .attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == key)?;
    attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
        .ok()
        .map(std::borrow::Cow::into_owned)
}

/// A site that needs JavaScript to show its images: hosts it matches, and its display name.
struct JsOnlySite {
    name: &'static str,
    /// `Brand("google")` matches `google.<any TLD>`; `Domain("x.com")` matches it and its
    /// subdomains.
    rule: HostRule,
}

enum HostRule {
    Brand(&'static str),
    Domain(&'static str),
}

const JS_ONLY_SITES: [JsOnlySite; 6] = [
    JsOnlySite {
        name: "Google",
        rule: HostRule::Brand("google"),
    },
    JsOnlySite {
        name: "Pinterest",
        rule: HostRule::Brand("pinterest"),
    },
    JsOnlySite {
        name: "Instagram",
        rule: HostRule::Domain("instagram.com"),
    },
    JsOnlySite {
        name: "X (Twitter)",
        rule: HostRule::Domain("x.com"),
    },
    JsOnlySite {
        name: "X (Twitter)",
        rule: HostRule::Domain("twitter.com"),
    },
    JsOnlySite {
        name: "Facebook",
        rule: HostRule::Domain("facebook.com"),
    },
];

/// Second-level labels of country domains like `google.com.ph` or `google.co.uk`.
const SECOND_LEVEL: [&str; 9] = ["com", "co", "org", "net", "edu", "gov", "ac", "or", "ne"];

/// Known JavaScript-only sites: Some(display name) when the host matches, e.g. Some("Google").
pub(crate) fn js_only_site(url: &Url) -> Option<&'static str> {
    let host = url.domain()?.trim_end_matches('.').to_ascii_lowercase();
    JS_ONLY_SITES
        .iter()
        .find(|site| match site.rule {
            HostRule::Domain(domain) => {
                host == domain
                    || host
                        .strip_suffix(domain)
                        .is_some_and(|sub| sub.ends_with('.'))
            }
            HostRule::Brand(brand) => is_brand_host(&host, brand),
        })
        .map(|site| site.name)
}

/// Whether `host` is `brand.<TLD>` or `brand.<second level>.<country>`, or a subdomain of one.
fn is_brand_host(host: &str, brand: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    let Some(at) = labels.iter().rposition(|l| *l == brand) else {
        return false;
    };
    match &labels[at + 1..] {
        [tld] => !tld.is_empty(),
        [second, country] => SECOND_LEVEL.contains(second) && country.len() == 2,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_parse() {
        for sel in [
            &BASE,
            &IMAGE_LINKS,
            &LINK_THUMB,
            &SRCSETS,
            &OG_IMAGE,
            &TWITTER_IMAGE,
            &IMG_SRC,
            &IMG_DATA_SRC,
            &PICTURE_IMG,
        ] {
            LazyLock::force(sel);
        }
    }

    fn page() -> Url {
        Url::parse("https://example.com/gallery/index.html").unwrap()
    }

    fn urls(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.url.as_str()).collect()
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    // ---- kind_of ----

    #[test]
    fn kind_of_should_trust_image_content_type() {
        assert_eq!(kind_of("image/jpeg", b"<html>"), PageKind::Image);
    }

    #[test]
    fn kind_of_should_trust_html_content_type_with_charset() {
        assert_eq!(
            kind_of("Text/HTML; charset=utf-8", b"\xFF\xD8\xFF"),
            PageKind::Html
        );
    }

    #[test]
    fn kind_of_should_treat_xhtml_as_html() {
        assert_eq!(kind_of("application/xhtml+xml", b""), PageKind::Html);
    }

    #[test]
    fn kind_of_should_treat_feed_and_xml_types_as_feed() {
        for ct in [
            "application/rss+xml",
            "application/atom+xml",
            "application/xml",
            "text/xml; charset=utf-8",
        ] {
            assert_eq!(kind_of(ct, b""), PageKind::Feed, "{ct}");
        }
    }

    #[test]
    fn kind_of_should_return_other_for_unknown_content_type() {
        assert_eq!(kind_of("application/json", b"<html>"), PageKind::Other);
    }

    #[test]
    fn kind_of_should_sniff_image_magic_when_content_type_empty() {
        for prefix in [
            &b"\xFF\xD8\xFF\xE0\0\x10JFIF"[..],
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR",
            b"RIFF\x16\0\0\0WEBPVP8X",
        ] {
            assert_eq!(kind_of("", prefix), PageKind::Image);
        }
    }

    #[test]
    fn kind_of_should_sniff_octet_stream() {
        assert_eq!(
            kind_of("application/octet-stream", b"\x89PNG\r\n\x1a\n"),
            PageKind::Image
        );
    }

    #[test]
    fn kind_of_should_sniff_rss_after_xml_declaration() {
        assert_eq!(
            kind_of(
                "",
                b"<?xml version=\"1.0\"?>\n<rss version=\"2.0\"><channel>"
            ),
            PageKind::Feed
        );
    }

    #[test]
    fn kind_of_should_sniff_atom_feed() {
        assert_eq!(
            kind_of(
                "",
                b"\xEF\xBB\xBF<?xml version='1.0'?><feed xmlns=\"http://www.w3.org/2005/Atom\">"
            ),
            PageKind::Feed
        );
    }

    #[test]
    fn kind_of_should_sniff_doctype_html() {
        assert_eq!(
            kind_of("", b"\n  <!DOCTYPE html><html lang=en>"),
            PageKind::Html
        );
    }

    #[test]
    fn kind_of_should_sniff_bare_html_tag() {
        assert_eq!(kind_of("", b"<HTML><body>"), PageKind::Html);
    }

    #[test]
    fn kind_of_should_return_other_for_unrecognised_bytes() {
        assert_eq!(kind_of("", b"{\"json\": true}"), PageKind::Other);
    }

    // ---- html_candidates ----

    #[test]
    fn html_should_take_links_to_image_files_case_insensitive() {
        let html = r#"<a href="a.JPG">a</a><a href="/b.jpeg?x=1">b</a><a href="c.html">c</a>
            <a href="https://cdn.example.net/d.Png">d</a><a href="e.webp#top">e</a>"#;
        assert_eq!(
            urls(&html_candidates(html, &page())),
            [
                "https://example.com/gallery/a.JPG",
                "https://example.com/b.jpeg?x=1",
                "https://cdn.example.net/d.Png",
                "https://example.com/gallery/e.webp",
            ]
        );
    }

    #[test]
    fn html_should_use_link_thumbnail_as_preview_and_not_as_candidate() {
        let html = r#"<a href="full.jpg"><img src="thumb.jpg" alt="Lake at dawn"></a>"#;
        assert_eq!(
            html_candidates(html, &page()),
            [Candidate {
                url: url("https://example.com/gallery/full.jpg"),
                preview: Some(url("https://example.com/gallery/thumb.jpg")),
                title: "Lake at dawn".into(),
            }]
        );
    }

    #[test]
    fn html_should_take_largest_srcset_entry_with_smallest_as_preview() {
        let html = r#"<img src="m.jpg" alt="Hills"
            srcset="s.jpg 480w, xl.jpg 3840w, m.jpg 1280w">"#;
        assert_eq!(
            html_candidates(html, &page()),
            [Candidate {
                url: url("https://example.com/gallery/xl.jpg"),
                preview: Some(url("https://example.com/gallery/s.jpg")),
                title: "Hills".into(),
            }]
        );
    }

    #[test]
    fn html_should_read_picture_source_and_data_srcset() {
        let html = r#"<picture><source srcset="p1.webp 800w, p2.webp 2400w"><img alt="Pic"></picture>
            <img data-srcset="lazy-small.jpg 300w, lazy-big.jpg 2000w">"#;
        let found = html_candidates(html, &page());
        assert_eq!(
            urls(&found),
            [
                "https://example.com/gallery/p2.webp",
                "https://example.com/gallery/lazy-big.jpg",
            ]
        );
    }

    #[test]
    fn html_should_title_picture_source_from_its_img_alt() {
        let html =
            r#"<picture><source srcset="p1.webp 800w, p2.webp 2400w"><img alt="Pic"></picture>"#;
        assert_eq!(html_candidates(html, &page())[0].title, "Pic");
    }

    #[test]
    fn html_should_keep_srcset_urls_with_commas() {
        let html = r#"<img srcset="https://img.example.com/w_400,h_300/a.jpg 400w, https://img.example.com/w_4000,h_3000/a.jpg 4000w">"#;
        assert_eq!(
            urls(&html_candidates(html, &page())),
            ["https://img.example.com/w_4000,h_3000/a.jpg"]
        );
    }

    #[test]
    fn html_should_take_og_and_twitter_images_after_links_and_srcset() {
        let html = r#"<head><meta name="twitter:image" content="/tw.jpg">
            <meta property="og:image" content="https://example.com/og.jpg"></head>
            <body><img src="plain.jpg"><a href="link.png">x</a></body>"#;
        assert_eq!(
            urls(&html_candidates(html, &page())),
            [
                "https://example.com/gallery/link.png",
                "https://example.com/og.jpg",
                "https://example.com/tw.jpg",
                "https://example.com/gallery/plain.jpg",
            ]
        );
    }

    #[test]
    fn html_should_take_img_src_then_data_src() {
        let html =
            r#"<img data-src="lazy.jpg" src="placeholder.png"><img src="b.jpg" title="Bee">"#;
        assert_eq!(
            urls(&html_candidates(html, &page())),
            [
                "https://example.com/gallery/placeholder.png",
                "https://example.com/gallery/b.jpg",
                "https://example.com/gallery/lazy.jpg",
            ]
        );
    }

    #[test]
    fn html_should_title_from_title_attribute() {
        let html = r#"<img src="b.jpg" title="Bee">"#;
        assert_eq!(html_candidates(html, &page())[0].title, "Bee");
    }

    #[test]
    fn html_should_title_from_decoded_file_name_without_alt() {
        let html = r#"<img src="/photos/Mount%20Fuji.jpg?w=1">"#;
        assert_eq!(html_candidates(html, &page())[0].title, "Mount Fuji.jpg");
    }

    #[test]
    fn html_should_resolve_against_base_href() {
        let html =
            r#"<head><base href="https://static.example.org/assets/"></head><img src="x.jpg">"#;
        assert_eq!(
            urls(&html_candidates(html, &page())),
            ["https://static.example.org/assets/x.jpg"]
        );
    }

    #[test]
    fn html_should_drop_non_http_schemes() {
        let html = r#"<img src="data:image/png;base64,AAAA"><img src="blob:https://example.com/1">
            <a href="javascript:void('x.jpg')">j</a><img src="ftp://example.com/a.jpg">
            <img src="http://plain.example.com/ok.jpg">"#;
        assert_eq!(
            urls(&html_candidates(html, &page())),
            ["http://plain.example.com/ok.jpg"]
        );
    }

    #[test]
    fn html_should_dedupe_ignoring_fragments() {
        let html = r#"<a href="a.jpg">1</a><a href="a.jpg#2">2</a><img src="a.jpg"><meta property="og:image" content="a.jpg">"#;
        assert_eq!(
            urls(&html_candidates(html, &page())),
            ["https://example.com/gallery/a.jpg"]
        );
    }

    #[test]
    fn html_should_cap_candidates() {
        let html: String = (0..100)
            .map(|i| format!(r#"<img src="img{i}.jpg">"#))
            .collect();
        assert_eq!(html_candidates(&html, &page()).len(), MAX_CANDIDATES);
    }

    #[test]
    fn html_should_skip_gif_svg_and_ico_files() {
        let html = r#"<img src="pixel.gif"><img src="logo.SVG"><img src="favicon.ico"><img src="real.jpg">"#;
        assert_eq!(
            urls(&html_candidates(html, &page())),
            ["https://example.com/gallery/real.jpg"]
        );
    }

    #[test]
    fn html_should_find_no_data_uris_on_google_like_page() {
        let html = r#"<!doctype html><html><head><script>var a='<img src="https://x.example/s.jpg">';</script>
            <style>body{background:url(data:image/png;base64,AAAA)}</style></head><body>
            <img src="data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==" width=1 height=1>
            <img src="/images/cleardot.gif" width=1 height=1>
            <img id="dimg_1" src="data:image/jpeg;base64,/9j/4AAQSkZJRg==" data-src="data:image/png;base64,iVBOR">
            <noscript><img src="data:image/png;base64,iVBOR"></noscript>
            <script>google.ldi={"dimg_1":"https://encrypted-tbn0.gstatic.com/images?q=tbn:abc"};</script>
            </body></html>"#;
        let found = html_candidates(html, &url("https://www.google.com/search?q=x&udm=2"));
        assert!(found.iter().all(|c| c.url.scheme() != "data"), "{found:?}");
    }

    #[test]
    fn html_should_ignore_urls_inside_scripts() {
        let html = r#"<script>var a='<img src="https://x.example/s.jpg">';</script>"#;
        assert_eq!(html_candidates(html, &page()), []);
    }

    // ---- feed_candidates ----

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title>Photos</title>
    <image><url>https://example.com/logo.png</url></image>
    <item>
      <title>Sunrise &amp; fog</title>
      <enclosure url="https://cdn.example.com/sunrise.jpg" type="image/jpeg" length="1"/>
    </item>
    <item>
      <title>Podcast</title>
      <enclosure url="https://cdn.example.com/episode.mp3" type="audio/mpeg" length="1"/>
    </item>
    <item>
      <title>No type</title>
      <enclosure url="/rel/peak.PNG"/>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn feed_should_take_rss_image_enclosures_only() {
        assert_eq!(
            urls(&feed_candidates(RSS, &url("https://example.com/feed.xml")).unwrap()),
            [
                "https://cdn.example.com/sunrise.jpg",
                "https://example.com/rel/peak.PNG"
            ]
        );
    }

    #[test]
    fn feed_should_title_enclosure_from_item_title() {
        let found = feed_candidates(RSS, &url("https://example.com/feed.xml")).unwrap();
        assert_eq!(found[0].title, "Sunrise & fog");
    }

    #[test]
    fn feed_should_take_atom_enclosure_links() {
        let xml = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
            <title>Atom</title><link href="https://example.com/"/>
            <entry><title>Big</title>
              <link rel="alternate" href="https://example.com/post.html"/>
              <link rel="enclosure" type="image/webp" href="https://cdn.example.com/big.webp"/>
            </entry></feed>"#;
        assert_eq!(
            feed_candidates(xml, &url("https://example.com/atom.xml")).unwrap(),
            [Candidate {
                url: url("https://cdn.example.com/big.webp"),
                preview: None,
                title: "Big".into(),
            }]
        );
    }

    #[test]
    fn feed_should_take_media_content_with_thumbnail_as_preview() {
        let xml = r#"<rss xmlns:media="http://search.yahoo.com/mrss/"><channel>
            <item><title>M</title>
              <media:content url="https://cdn.example.com/m.jpg" medium="image"/>
              <media:content url="https://cdn.example.com/v.mp4" medium="video"/>
              <media:content url="https://cdn.example.com/typed" type="image/png"/>
              <media:thumbnail url="https://cdn.example.com/m-thumb.jpg"/>
            </item></channel></rss>"#;
        assert_eq!(
            feed_candidates(xml, &url("https://example.com/feed")).unwrap(),
            [
                Candidate {
                    url: url("https://cdn.example.com/m.jpg"),
                    preview: Some(url("https://cdn.example.com/m-thumb.jpg")),
                    title: "M".into(),
                },
                Candidate {
                    url: url("https://cdn.example.com/typed"),
                    preview: Some(url("https://cdn.example.com/m-thumb.jpg")),
                    title: "M".into(),
                },
            ]
        );
    }

    #[test]
    fn feed_should_not_use_media_thumbnail_alone_as_candidate() {
        let xml = r#"<rss xmlns:media="http://search.yahoo.com/mrss/"><channel>
            <item><media:thumbnail url="https://cdn.example.com/t.jpg"/></item></channel></rss>"#;
        assert_eq!(
            feed_candidates(xml, &url("https://example.com/feed")).unwrap(),
            []
        );
    }

    #[test]
    fn feed_should_take_images_in_escaped_description_html() {
        let xml = r#"<rss><channel><item><title>Post</title>
            <description>&lt;p&gt;Look:&lt;/p&gt;&lt;img src="https://cdn.example.com/d.jpg" alt="Dune"&gt;</description>
            </item></channel></rss>"#;
        let found = feed_candidates(xml, &url("https://example.com/feed")).unwrap();
        assert_eq!(urls(&found), ["https://cdn.example.com/d.jpg"]);
    }

    #[test]
    fn feed_should_take_images_in_cdata_content_encoded() {
        let xml = r#"<rss xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel><item>
            <content:encoded><![CDATA[<a href="/full/e.jpg"><img src="/thumb/e.jpg"></a>]]></content:encoded>
            </item></channel></rss>"#;
        assert_eq!(
            feed_candidates(xml, &url("https://example.com/blog/feed")).unwrap(),
            [Candidate {
                url: url("https://example.com/full/e.jpg"),
                preview: Some(url("https://example.com/thumb/e.jpg")),
                title: "e.jpg".into(),
            }]
        );
    }

    #[test]
    fn feed_should_dedupe_and_drop_non_http() {
        let xml = r#"<rss><channel>
            <item><enclosure url="https://cdn.example.com/a.jpg" type="image/jpeg"/></item>
            <item><enclosure url="https://cdn.example.com/a.jpg#x" type="image/jpeg"/></item>
            <item><enclosure url="data:image/jpeg;base64,AAAA" type="image/jpeg"/></item>
            </channel></rss>"#;
        assert_eq!(
            urls(&feed_candidates(xml, &url("https://example.com/feed")).unwrap()),
            ["https://cdn.example.com/a.jpg"]
        );
    }

    #[test]
    fn feed_should_fail_on_mismatched_tags() {
        let xml = "<rss><channel><item></channel></rss>";
        assert_eq!(
            feed_candidates(xml, &page()).unwrap_err().to_string(),
            "web: can't read this feed"
        );
    }

    #[test]
    fn feed_should_fail_on_unclosed_document() {
        assert!(feed_candidates("<rss><channel><item>", &page()).is_err());
    }

    #[test]
    fn feed_should_fail_on_html() {
        assert!(feed_candidates("<!DOCTYPE html><html><body><br></body></html>", &page()).is_err());
    }

    #[test]
    fn feed_should_fail_on_empty_document() {
        assert!(feed_candidates("   ", &page()).is_err());
    }

    // ---- js_only_site ----

    #[test]
    fn js_only_site_should_match_google_on_any_tld() {
        for host in [
            "https://google.com/",
            "https://www.google.com/search?q=x",
            "https://www.google.com.ph/search?q=x",
            "https://images.google.co.uk/",
            "https://www.google.de/",
            "https://WWW.GOOGLE.COM./",
        ] {
            assert_eq!(js_only_site(&url(host)), Some("Google"), "{host}");
        }
    }

    #[test]
    fn js_only_site_should_match_pinterest_on_any_tld() {
        for host in [
            "https://www.pinterest.com/pin/1",
            "https://pinterest.co.uk/",
            "https://fr.pinterest.fr/",
        ] {
            assert_eq!(js_only_site(&url(host)), Some("Pinterest"), "{host}");
        }
    }

    #[test]
    fn js_only_site_should_match_fixed_hosts() {
        assert_eq!(
            js_only_site(&url("https://www.instagram.com/p/x")),
            Some("Instagram")
        );
        assert_eq!(
            js_only_site(&url("https://x.com/user")),
            Some("X (Twitter)")
        );
        assert_eq!(
            js_only_site(&url("https://mobile.twitter.com/user")),
            Some("X (Twitter)")
        );
        assert_eq!(
            js_only_site(&url("https://m.facebook.com/photo")),
            Some("Facebook")
        );
    }

    #[test]
    fn js_only_site_should_respect_label_boundaries() {
        for host in [
            "https://notgoogle.com/",
            "https://google.example.com/",
            "https://mygoogle.co.uk/",
            "https://fox.com/",
            "https://notinstagram.com/",
            "https://instagram.com.evil.example/",
            "https://example.com/google.com",
        ] {
            assert_eq!(js_only_site(&url(host)), None, "{host}");
        }
    }

    #[test]
    fn js_only_site_should_ignore_ip_hosts() {
        assert_eq!(js_only_site(&url("https://127.0.0.1/")), None);
    }
}
