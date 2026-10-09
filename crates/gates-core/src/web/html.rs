//! A page's HTML as plain readable text for a model: scripts, styles and
//! page furniture (menus, footers, forms' buttons) dropped, headings kept as
//! `#` lines, list items as `- `, links as `[text](https://…)` with their
//! target made absolute. No dependency: a small tolerant tokenizer, since
//! the pages are untrusted and badly formed.
//!
//! The text still comes from the internet: it goes to the model as data
//! (`web::untrusted`), and invisible and direction-changing characters are
//! taken out so a page can't hide words from the user's eye in it.

use super::is_invisible as invisible;
use url::Url;

/// A page, reduced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extracted {
    pub title: String,
    pub text: String,
}

/// `html` as text. Relative links are resolved against `base`; only http
/// and https ones are kept.
pub fn extract(html: &str, base: &Url) -> Extracted {
    let first = Reader::run(html, base, false);
    // A page with a <main> is its content; the rest around it is furniture.
    if first.saw_main {
        let second = Reader::run(html, base, true);
        if !second.text.trim().is_empty() {
            return second.finish();
        }
    }
    first.finish()
}

/// Elements whose content is not the page's text.
const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "iframe", "object", "canvas", "nav",
    "footer", "aside", "select", "button", "dialog", "head",
];

/// Elements whose content is text up to their closing tag, not markup.
const RAW: &[&str] = &["script", "style", "textarea", "title", "noscript"];

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// A break of a paragraph's size before and after.
const PARAGRAPH: &[&str] = &[
    "p",
    "section",
    "article",
    "main",
    "header",
    "blockquote",
    "ul",
    "ol",
    "table",
    "dl",
    "pre",
    "figure",
    "hr",
    "details",
    "address",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
];

/// A break of a line before and after.
const LINE: &[&str] = &[
    "br",
    "div",
    "li",
    "tr",
    "dt",
    "dd",
    "caption",
    "figcaption",
    "summary",
];

struct Reader<'a> {
    base: &'a Url,
    /// Only text inside <main> counts.
    focus: bool,
    saw_main: bool,
    main_depth: u32,
    text: String,
    title: String,
    /// A space is owed before the next word.
    space: bool,
    pre: u32,
    /// The element being skipped, and how deep its own kind nests in it.
    skipping: Option<(String, u32)>,
    /// Open links: where their text starts, and where they point.
    links: Vec<(usize, Option<String>)>,
}

impl<'a> Reader<'a> {
    fn run(html: &str, base: &'a Url, focus: bool) -> Reader<'a> {
        let mut r = Reader {
            base,
            focus,
            saw_main: false,
            main_depth: 0,
            text: String::new(),
            title: String::new(),
            space: false,
            pre: 0,
            skipping: None,
            links: Vec::new(),
        };
        r.read(html);
        r
    }

    fn finish(self) -> Extracted {
        Extracted {
            title: tidy_line(&self.title),
            text: tidy(&self.text),
        }
    }

    fn counting(&self) -> bool {
        self.skipping.is_none() && (!self.focus || self.main_depth > 0)
    }

    fn read(&mut self, html: &str) {
        let bytes = html.as_bytes();
        let mut i = 0;
        let mut start = 0;
        while i < bytes.len() {
            if bytes[i] != b'<' {
                i += 1;
                continue;
            }
            let Some(&next) = bytes.get(i + 1) else {
                break;
            };
            let markup = next == b'/' || next == b'!' || next == b'?' || next.is_ascii_alphabetic();
            if !markup {
                i += 1;
                continue;
            }
            self.text_run(&html[start..i]);
            i = self.markup(html, i);
            start = i;
        }
        if start < html.len() {
            self.text_run(&html[start..]);
        }
    }

    /// The tag, comment or declaration at `at`; the index after it.
    fn markup(&mut self, html: &str, at: usize) -> usize {
        let bytes = html.as_bytes();
        let rest = &html[at..];
        if rest.starts_with("<!--") {
            return rest.find("-->").map_or(html.len(), |e| at + e + 3);
        }
        if rest.starts_with("<![CDATA[") {
            return rest.find("]]>").map_or(html.len(), |e| at + e + 3);
        }
        if bytes[at + 1] == b'!' || bytes[at + 1] == b'?' {
            return rest.find('>').map_or(html.len(), |e| at + e + 1);
        }
        let closing = bytes[at + 1] == b'/';
        let name_start = at + if closing { 2 } else { 1 };
        let mut j = name_start;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b':' || bytes[j] == b'-')
        {
            j += 1;
        }
        let name = html[name_start..j].to_ascii_lowercase();
        if name.is_empty() {
            // "</>" or "</ x": nothing a browser would show; step over it.
            return rest.find('>').map_or(html.len(), |e| at + e + 1);
        }
        // Attributes, up to the '>' that isn't in quotes.
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut self_closing = false;
        loop {
            while j < bytes.len() && (bytes[j].is_ascii_whitespace() || bytes[j] == b'/') {
                self_closing = bytes[j] == b'/';
                j += 1;
            }
            if j >= bytes.len() {
                break;
            }
            if bytes[j] == b'>' {
                j += 1;
                break;
            }
            let key_start = j;
            while j < bytes.len()
                && !bytes[j].is_ascii_whitespace()
                && bytes[j] != b'='
                && bytes[j] != b'>'
                && bytes[j] != b'/'
            {
                j += 1;
            }
            let key = html[key_start..j].to_ascii_lowercase();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let mut value = String::new();
            if j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                    let quote = bytes[j];
                    let from = j + 1;
                    j = from;
                    while j < bytes.len() && bytes[j] != quote {
                        j += 1;
                    }
                    value = decode(&html[from..j.min(html.len())]);
                    j = (j + 1).min(bytes.len());
                } else {
                    let from = j;
                    while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'>' {
                        j += 1;
                    }
                    value = decode(&html[from..j]);
                }
            }
            if !key.is_empty() {
                attrs.push((key, value));
            } else if j == key_start {
                // No progress (nothing a tag can hold): step over one whole
                // character, never into the middle of one.
                j += html[j..].chars().next().map_or(1, char::len_utf8);
            }
            j = j.min(bytes.len());
        }
        if closing {
            self.end(&name);
            return j;
        }
        self.start(&name, &attrs, self_closing);
        // Text up to the closing tag, for elements whose content isn't markup.
        if RAW.contains(&name.as_str()) && !self_closing {
            let end = find_closing(html, j, &name).unwrap_or(html.len());
            if name == "title" {
                if self.title.is_empty() {
                    self.title = decode(&html[j..end]);
                }
            } else if name == "textarea" && self.counting() {
                self.text_run(&html[j..end]);
            }
            // Script, style and noscript text is dropped.
            let after = html[end..].find('>').map_or(html.len(), |e| end + e + 1);
            if end < html.len() {
                self.end(&name);
            }
            return after;
        }
        j
    }

    fn start(&mut self, name: &str, attrs: &[(String, String)], self_closing: bool) {
        let void = VOID.contains(&name) || self_closing;
        // <head> ends where <body> begins, closed or not.
        if name == "body" && matches!(&self.skipping, Some((s, _)) if s == "head") {
            self.skipping = None;
        }
        if let Some((skipped, depth)) = self.skipping.as_mut() {
            if skipped == name && !void {
                *depth += 1;
            }
            return;
        }
        let attr = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        let hidden = attrs.iter().any(|(a, _)| a == "hidden")
            || attr("aria-hidden") == Some("true")
            || attr("style").is_some_and(|s| {
                let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
                let s = s.to_ascii_lowercase();
                s.contains("display:none") || s.contains("visibility:hidden")
            });
        if (SKIPPED.contains(&name) || hidden) && !void {
            // <head> ends where <body> begins even when it isn't closed.
            self.skipping = Some((name.to_string(), 1));
            return;
        }
        if name == "main" {
            self.saw_main = true;
            self.main_depth += 1;
        }
        if name == "body" {
            return;
        }
        if !self.counting() {
            if name == "a" {
                self.links.push((self.text.len(), None));
            }
            return;
        }
        match name {
            "a" => {
                self.flush_space();
                let target = attr("href").and_then(|h| self.target(h));
                self.links.push((self.text.len(), target));
            }
            "pre" => {
                self.paragraph();
                self.pre += 1;
            }
            "li" => {
                self.line();
                self.text.push_str("- ");
                self.space = false;
            }
            "td" | "th" => {
                if !self.text.is_empty() && !self.text.ends_with('\n') {
                    self.text.push_str(" | ");
                    self.space = false;
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.paragraph();
                let level = name.as_bytes()[1] - b'0';
                for _ in 0..level {
                    self.text.push('#');
                }
                self.text.push(' ');
                self.space = false;
            }
            _ => {
                if PARAGRAPH.contains(&name) {
                    self.paragraph();
                } else if LINE.contains(&name) {
                    self.line();
                }
            }
        }
    }

    fn end(&mut self, name: &str) {
        if let Some((skipped, depth)) = self.skipping.as_mut() {
            if skipped == name {
                *depth -= 1;
                if *depth == 0 {
                    self.skipping = None;
                }
            }
            return;
        }
        match name {
            "main" => self.main_depth = self.main_depth.saturating_sub(1),
            "a" => self.close_link(),
            "pre" => {
                self.pre = self.pre.saturating_sub(1);
                self.paragraph();
            }
            _ if !self.counting() => {}
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.paragraph(),
            _ => {
                if PARAGRAPH.contains(&name) {
                    self.paragraph();
                } else if LINE.contains(&name) {
                    self.line();
                }
            }
        }
    }

    /// The link's text so far becomes `[text](target)`.
    fn close_link(&mut self) {
        let Some((from, target)) = self.links.pop() else {
            return;
        };
        let Some(target) = target else {
            return;
        };
        if from > self.text.len() || !self.text.is_char_boundary(from) {
            return;
        }
        let label = self.text[from..].trim().replace(['[', ']'], "");
        if label.is_empty() {
            return;
        }
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        self.text.truncate(from);
        self.text.push_str(&format!("[{label}]({target})"));
    }

    /// A link's address, absolute; None for one that isn't a web page.
    fn target(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() || href.starts_with('#') {
            return None;
        }
        let mut url = self.base.join(href).ok()?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return None;
        }
        url.set_fragment(None);
        // Parentheses would end the Markdown link early.
        Some(url.as_str().replace('(', "%28").replace(')', "%29"))
    }

    fn flush_space(&mut self) {
        if self.space && !self.text.is_empty() && !self.text.ends_with(['\n', ' ']) {
            self.text.push(' ');
        }
        self.space = false;
    }

    /// Ends the line, if there is anything on it.
    fn line(&mut self) {
        self.space = false;
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    /// Ends the line and leaves an empty one.
    fn paragraph(&mut self) {
        self.line();
        if !self.text.is_empty() && !self.text.ends_with("\n\n") {
            self.text.push('\n');
        }
    }

    fn text_run(&mut self, raw: &str) {
        if raw.is_empty() || !self.counting() {
            return;
        }
        let decoded = decode(raw);
        if self.pre > 0 {
            self.text.push_str(&decoded);
            return;
        }
        for c in decoded.chars() {
            if c.is_whitespace() || c == '\u{a0}' {
                self.space = true;
            } else {
                self.flush_space();
                self.text.push(c);
            }
        }
    }
}

/// One line of text: no invisible characters, spaces collapsed.
fn tidy_line(text: &str) -> String {
    text.chars()
        .filter(|c| !invisible(*c) || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where `</name` (any case) first appears in `html` from `from`, without
/// copying the page: a page of thousands of raw-text tags stays linear.
fn find_closing(html: &str, from: usize, name: &str) -> Option<usize> {
    let bytes = html.as_bytes();
    let needle = name.as_bytes();
    let mut i = from;
    while i + 2 + needle.len() <= bytes.len() {
        i += bytes[i..].iter().position(|&b| b == b'<')?;
        if i + 2 + needle.len() <= bytes.len()
            && bytes[i + 1] == b'/'
            && bytes[i + 2..i + 2 + needle.len()].eq_ignore_ascii_case(needle)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// A list item that is only a link: `- [label](address)`.
fn bare_link(line: &str) -> bool {
    line.starts_with("- [")
        && line.ends_with(')')
        && line.matches("](").count() == 1
        && line.chars().count() < 400
}

/// Menus and language lists are runs of nothing but links, and can fill the
/// page's whole allowance (a Wikipedia article starts with 90 languages). A
/// run of more than `KEEP` of them is cut to its first few and a count.
fn squeeze_links(text: &str) -> String {
    const KEEP: usize = 4;
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if !bare_link(lines[i]) {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        let mut end = i;
        while end < lines.len() && bare_link(lines[end]) {
            end += 1;
        }
        let run = end - i;
        if run > KEEP + 2 {
            out.extend(lines[i..i + KEEP].iter().map(|l| l.to_string()));
            out.push(format!("- … {} more links", run - KEEP));
        } else {
            out.extend(lines[i..end].iter().map(|l| l.to_string()));
        }
        i = end;
    }
    out.join("\n")
}

/// The text's lines trimmed, runs of empty lines made one, invisible
/// characters taken out.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut empty = 0;
    for line in text.lines() {
        let line: String = line.chars().filter(|c| !invisible(*c)).collect();
        let line = line.trim_end().replace('\t', "    ");
        if line.trim().is_empty() {
            empty += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if empty > 0 {
                out.push('\n');
            }
        }
        empty = 0;
        out.push_str(line.trim_end());
    }
    squeeze_links(&out)
}

/// Character references decoded: the common named ones, and numbers.
pub fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // A reference is short and ends in ';'.
        let end = rest.char_indices().take(12).find(|(_, c)| *c == ';');
        let Some((end, _)) = end else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        match reference(name) {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn reference(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let value = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        let c = char::from_u32(value)?;
        return (!c.is_control() || c == '\n' || c == '\t').then_some(c);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "middot" => '·',
        "times" => '×',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "deg" => '°',
        "plusmn" => '±',
        "frac12" => '½',
        "larr" => '←',
        "rarr" => '→',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(html: &str) -> String {
        extract(
            html,
            &Url::parse("https://example.org/docs/page.html").unwrap(),
        )
        .text
    }

    #[test]
    fn scripts_and_styles_go_headings_and_links_stay() {
        let html = r#"<html><head><title> The  Title &amp; more </title>
            <style>body { color: red }</style><script>var x = "<p>not text</p>";</script></head>
            <body><h1>Big</h1><p>Some <b>bold</b> words and a
            <a href="/guide/start?x=1#top">link</a>.</p>
            <h3>Small</h3><ul><li>one</li><li>two</li></ul>
            <script>alert(1)</script></body></html>"#;
        let got = extract(
            html,
            &Url::parse("https://example.org/docs/page.html").unwrap(),
        );
        assert_eq!(got.title, "The Title & more");
        assert_eq!(
            got.text,
            "# Big\n\nSome bold words and a [link](https://example.org/guide/start?x=1).\n\n### Small\n\n- one\n- two"
        );
        assert!(!got.text.contains("alert") && !got.text.contains("color"));
    }

    #[test]
    fn furniture_is_dropped_and_main_wins() {
        let html = "<body><nav><a href='/a'>Menu</a></nav><div>Outside</div>\
                    <main><p>Inside the main.</p><aside>Side</aside></main>\
                    <footer>Copyright</footer></body>";
        assert_eq!(text(html), "Inside the main.");
        // Without a <main>, the body counts, minus the furniture.
        let plain = "<body><nav>Menu</nav><p>One</p><footer>Foot</footer><p>Two</p></body>";
        assert_eq!(text(plain), "One\n\nTwo");
    }

    #[test]
    fn links_are_made_absolute_and_only_web_ones_stay() {
        let html = "<p><a href='../up'>up</a> <a href='https://other.net/x y'>far</a> \
                    <a href='javascript:evil()'>js</a> <a href='mailto:a@b.c'>mail</a> \
                    <a href='#top'>top</a> <a href='//cdn.example.net/f'>proto</a> \
                    <a href='/a(b)'>paren</a> <a href='/e'></a></p>";
        assert_eq!(
            text(html),
            "[up](https://example.org/up) [far](https://other.net/x%20y) js mail top \
             [proto](https://cdn.example.net/f) [paren](https://example.org/a%28b%29)"
        );
    }

    #[test]
    fn hidden_things_and_odd_markup() {
        let html = "<p>seen</p><div hidden>unseen</div><div style='display: none'>nope</div>\
                    <span aria-hidden=\"true\">icon</span><!-- a <b>comment</b> -->\
                    <p>a &lt; b &#38; c &#x41; &bogus; &amp</p><br/><p>5 < 6 and <3</p>\
                    <div><div>deep</div>after</div><unclosed";
        assert_eq!(
            text(html),
            "seen\n\na < b & c A &bogus; &amp\n\n5 < 6 and <3\n\ndeep\nafter"
        );
    }

    #[test]
    fn tables_and_preformatted_text() {
        let html = "<table><tr><th>Name</th><th>Size</th></tr><tr><td>a</td><td>1</td></tr></table>\
                    <pre>line 1\n  line 2</pre>";
        assert_eq!(text(html), "Name | Size\na | 1\n\nline 1\n  line 2");
    }

    #[test]
    fn odd_attributes_never_panic() {
        // An attribute with no name, then a multibyte character.
        assert_eq!(text("<p><a =\"x\"é>link</a> after</p>"), "link after");
        assert_eq!(text("<a =x é>é</a>"), "é");
        // The same at the end of the input.
        assert_eq!(text("<p>seen</p><title =\"x\""), "seen");
        assert_eq!(text("<title ="), "");
        assert_eq!(text("<p x=\"unclosed é"), "");
        assert_eq!(text("<a href=é"), "");
        // Whatever the bytes, the reader reaches the end without a panic.
        let nasty = "<<a =\"é<b ='<title =é<//é</é<!-<? = / \"'>é<style ='é";
        for cut in 0..nasty.len() {
            if nasty.is_char_boundary(cut) {
                let _ = text(&nasty[..cut]);
            }
        }
    }

    #[test]
    fn a_page_of_raw_tags_is_read_in_linear_time() {
        let page = format!(
            "<p>start</p>{}<p>end</p>",
            "<style></style>".repeat(100_000)
        );
        assert!(page.len() > 1_500_000);
        let started = std::time::Instant::now();
        assert_eq!(text(&page), "start\n\nend");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        // Mixed case closes them too, and an unclosed one runs to the end once.
        assert_eq!(text("<STYLE>x</Style><p>a</p><script>never"), "a");
    }

    #[test]
    fn runs_of_bare_links_are_cut_short() {
        let mut html = String::from("<main><h1>Article</h1><ul>");
        for i in 0..40 {
            html.push_str(&format!("<li><a href='/lang/{i}'>Language {i}</a></li>"));
        }
        html.push_str("</ul><p>The text.</p><ul><li><a href='/a'>One</a></li><li><a href='/b'>Two</a></li></ul></main>");
        let got = text(&html);
        assert!(got.contains("- [Language 3](https://example.org/lang/3)"));
        assert!(!got.contains("Language 4]"));
        assert!(got.contains("- … 36 more links\n\nThe text."));
        // A short list of links is kept whole.
        assert!(got.contains("- [One](https://example.org/a)\n- [Two](https://example.org/b)"));
    }

    #[test]
    fn invisible_characters_are_taken_out() {
        let html = "<p>sa\u{200b}fe \u{202e}text\u{feff} &#8238;x &#7; end</p>";
        // A control character written as a number stays as the text it is.
        assert_eq!(text(html), "safe text x &#7; end");
    }

    #[test]
    fn a_page_that_is_all_one_unclosed_element_still_reads() {
        // An unclosed <script> runs to the end, as in a browser.
        assert_eq!(text("<p>head</p><script>never closed <p>x</p>"), "head");
        // An unclosed <nav> is the page's furniture, but the rest isn't lost
        // when it closes later.
        assert_eq!(text("<nav>menu</nav><p>body</p>"), "body");
    }

    #[test]
    fn character_references() {
        assert_eq!(
            decode("a&nbsp;b &mdash; &copy; &#169; &#xA9; &#0;"),
            "a b — © © © &#0;"
        );
        assert_eq!(decode("&&&;"), "&&&;");
        assert_eq!(decode("no references"), "no references");
    }
}
