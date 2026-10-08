//! A reply's Markdown, as the window shows it: prose as a small, safe subset
//! of HTML that Qt's rich text draws, and each fenced or indented code block
//! on its own, as plain text, for a code view with a Copy button.
//!
//! Replies come from a model: nothing in them is trusted. Every piece of text
//! is escaped, raw HTML is shown as the text it is, images are shown as their
//! description (never fetched), and a link keeps its target only when it is
//! http, https or mailto.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Rich text for Qt (`Text.RichText`).
    Prose(String),
    /// A code block's text, as written, and its language ("" for none).
    Code { lang: String, code: String },
}

/// `text` as blocks, in order. Unfinished Markdown (a reply still coming in)
/// renders as far as it goes: an open code fence is a code block to the end.
pub fn blocks(text: &str) -> Vec<Block> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut out = Vec::new();
    let mut html = String::new();
    // Inside a code block: its language, its text so far, and whether it
    // stays in the prose (see `nest`).
    let mut code: Option<(String, String, bool)> = None;
    // Open lists and quotes. A code block inside one stays in the prose, as
    // <pre>: splitting it out would cut the list or quote in two.
    let mut nest = 0u32;
    // Whether each open link was written as <a> (only safe targets are).
    let mut links: Vec<bool> = Vec::new();
    // In a table's head row: cells are <th>.
    let mut in_head = false;

    for event in Parser::new_ext(text, options) {
        if let Some((_, buf, _)) = code.as_mut() {
            match event {
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, mut code_text, inline) = code.take().unwrap_or_default();
                    if code_text.ends_with('\n') {
                        code_text.pop();
                    }
                    if inline {
                        pre_into(&mut html, &code_text);
                    } else {
                        out.push(Block::Code {
                            lang,
                            code: code_text,
                        });
                    }
                }
                Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                    buf.push_str(&t)
                }
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::CodeBlock(kind) => {
                    let inline = nest > 0;
                    if !inline {
                        flush(&mut out, &mut html);
                    }
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => {
                            info.split_whitespace().next().unwrap_or("").to_string()
                        }
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new(), inline));
                }
                Tag::Paragraph => html.push_str("<p>"),
                Tag::Heading { level, .. } => {
                    html.push_str(heading(level).0);
                }
                Tag::BlockQuote(_) => {
                    nest += 1;
                    html.push_str("<blockquote>")
                }
                Tag::List(Some(start)) => {
                    nest += 1;
                    html.push_str(&format!("<ol start=\"{start}\">"))
                }
                Tag::List(None) => {
                    nest += 1;
                    html.push_str("<ul>")
                }
                Tag::Item => html.push_str("<li>"),
                Tag::Emphasis => html.push_str("<i>"),
                Tag::Strong => html.push_str("<b>"),
                Tag::Strikethrough => html.push_str("<s>"),
                Tag::Link { dest_url, .. } => {
                    let safe = safe_link(&dest_url);
                    if safe {
                        html.push_str("<a href=\"");
                        escape_into(&mut html, &dest_url);
                        html.push_str("\">");
                    }
                    links.push(safe);
                }
                Tag::Table(_) => {
                    html.push_str("<table border=\"1\" cellspacing=\"0\" cellpadding=\"4\">")
                }
                Tag::TableHead => {
                    in_head = true;
                    html.push_str("<tr>");
                }
                Tag::TableRow => html.push_str("<tr>"),
                Tag::TableCell => html.push_str(if in_head { "<th>" } else { "<td>" }),
                // An image is its description, which comes as its text.
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => html.push_str("</p>"),
                TagEnd::Heading(level) => html.push_str(heading(level).1),
                TagEnd::BlockQuote(_) => {
                    nest = nest.saturating_sub(1);
                    html.push_str("</blockquote>")
                }
                TagEnd::List(ordered) => {
                    nest = nest.saturating_sub(1);
                    html.push_str(if ordered { "</ol>" } else { "</ul>" })
                }
                TagEnd::Item => html.push_str("</li>"),
                TagEnd::Emphasis => html.push_str("</i>"),
                TagEnd::Strong => html.push_str("</b>"),
                TagEnd::Strikethrough => html.push_str("</s>"),
                TagEnd::Link => {
                    if links.pop().unwrap_or(false) {
                        html.push_str("</a>");
                    }
                }
                TagEnd::Table => html.push_str("</table>"),
                TagEnd::TableHead => {
                    in_head = false;
                    html.push_str("</tr>");
                }
                TagEnd::TableRow => html.push_str("</tr>"),
                TagEnd::TableCell => html.push_str(if in_head { "</th>" } else { "</td>" }),
                _ => {}
            },
            Event::Text(t) => escape_into(&mut html, &t),
            Event::Code(t) => {
                html.push_str("<code>");
                escape_into(&mut html, &t);
                html.push_str("</code>");
            }
            // Raw HTML from the reply is shown, never interpreted.
            Event::Html(t) | Event::InlineHtml(t) => escape_into(&mut html, &t),
            Event::SoftBreak => html.push(' '),
            Event::HardBreak => html.push_str("<br/>"),
            Event::Rule => html.push_str("<hr/>"),
            Event::TaskListMarker(done) => html.push_str(if done { "☑ " } else { "☐ " }),
            Event::FootnoteReference(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                escape_into(&mut html, &t)
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
    if let Some((lang, code_text, inline)) = code {
        let code_text = code_text.trim_end_matches('\n');
        if inline {
            pre_into(&mut html, code_text);
        } else {
            out.push(Block::Code {
                lang,
                code: code_text.to_string(),
            });
        }
    }
    flush(&mut out, &mut html);
    out
}

fn pre_into(html: &mut String, code: &str) {
    html.push_str("<pre>");
    escape_into(html, code);
    html.push_str("</pre>");
}

fn flush(out: &mut Vec<Block>, html: &mut String) {
    if !html.trim().is_empty() {
        out.push(Block::Prose(std::mem::take(html)));
    }
    html.clear();
}

/// Replies' headings are a step smaller than a page's: the window has its own.
fn heading(level: HeadingLevel) -> (&'static str, &'static str) {
    match level {
        HeadingLevel::H1 => ("<h3>", "</h3>"),
        HeadingLevel::H2 => ("<h4>", "</h4>"),
        _ => ("<h5>", "</h5>"),
    }
}

fn safe_link(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

/// Escapes `&`, `<`, `>`, `"` and `'`.
pub fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// Text for a user's own message: escaped, with its line breaks kept.
pub fn plain_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push_str("<br/>");
        }
        escape_into(&mut out, line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose(text: &str) -> String {
        blocks(text)
            .into_iter()
            .filter_map(|b| match b {
                Block::Prose(h) => Some(h),
                Block::Code { .. } => None,
            })
            .collect()
    }

    #[test]
    fn basic_markup() {
        assert_eq!(
            prose("Hello **bold** and *it* `x<y`"),
            "<p>Hello <b>bold</b> and <i>it</i> <code>x&lt;y</code></p>"
        );
    }

    #[test]
    fn code_blocks_are_split_out() {
        let b = blocks("Before\n\n```rust\nfn main() {}\n```\n\nAfter");
        assert_eq!(b.len(), 3);
        assert_eq!(
            b[1],
            Block::Code {
                lang: "rust".into(),
                code: "fn main() {}".into()
            }
        );
        assert_eq!(b[2], Block::Prose("<p>After</p>".into()));
    }

    #[test]
    fn unfinished_fence_is_code_to_the_end() {
        let b = blocks("Here:\n```py\nprint(1)\nprint(2");
        assert_eq!(
            b.last(),
            Some(&Block::Code {
                lang: "py".into(),
                code: "print(1)\nprint(2".into()
            })
        );
    }

    #[test]
    fn raw_html_is_text() {
        let h = prose("<img src=x onerror=alert(1)> <b>hi</b>\n\n<script>bad()</script>");
        assert!(!h.contains("<img"));
        assert!(!h.contains("<script"));
        assert!(h.contains("&lt;script&gt;"));
        assert!(h.contains("&lt;b&gt;hi&lt;/b&gt;"));
    }

    #[test]
    fn images_are_their_description() {
        let h = prose("![a cat](https://example.com/cat.png)");
        assert_eq!(h, "<p>a cat</p>");
    }

    #[test]
    fn only_web_links_keep_their_target() {
        assert!(
            prose("[ok](https://example.com)").contains("<a href=\"https://example.com\">ok</a>")
        );
        let bad = prose("[bad](javascript:alert(1)) [file](file:///etc/passwd)");
        assert!(!bad.contains("<a"));
        assert!(bad.contains("bad"));
    }

    #[test]
    fn link_targets_are_escaped() {
        let h = prose("[x](https://e.com/\"onmouseover=\"y)");
        assert!(!h.contains("\"onmouseover"));
    }

    #[test]
    fn lists_tables_and_quotes() {
        let h = prose(
            "1. one\n2. two\n\n- a\n- [x] done\n\n> quoted\n\n| A | B |\n|---|---|\n| 1 | 2 |",
        );
        assert!(h.contains("<ol start=\"1\"><li>one</li>"));
        assert!(h.contains("<li>☑ done</li>"));
        assert!(h.contains("<blockquote><p>quoted</p></blockquote>"));
        assert!(h.contains("<tr><th>A</th><th>B</th></tr>"));
        assert!(h.contains("<tr><td>1</td><td>2</td></tr>"));
    }

    #[test]
    fn code_in_a_list_stays_in_the_list() {
        let b = blocks("1. Install:\n\n   ```sh\n   dnf install <x>\n   ```\n\n2. Run");
        assert_eq!(b.len(), 1, "{b:?}");
        let Block::Prose(h) = &b[0] else {
            panic!("{b:?}")
        };
        assert!(h.contains("<pre>dnf install &lt;x&gt;</pre>"));
        assert!(h.contains("<li><p>Run</p></li></ol>"));
    }

    #[test]
    fn plain_html_escapes_and_breaks() {
        assert_eq!(plain_html("a<b>\nc"), "a&lt;b&gt;<br/>c");
    }

    #[test]
    fn empty_text_is_no_blocks() {
        assert!(blocks("").is_empty());
        assert!(blocks("   \n").is_empty());
    }
}
