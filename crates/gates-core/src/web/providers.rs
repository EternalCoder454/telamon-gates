//! The search services: what to send each and how to read its answer. The
//! request is built and the answer parsed here with no network (so both are
//! tested from sample responses); `send` is the one function that goes out.
//!
//! The API key travels in a header, never in the address, so it can't end up
//! in an error message, a log or a redirect.

use super::{Provider, SearchResult, WebError, html};
use serde_json::{Value, json};
use std::io::Read;
use std::time::Duration;
use url::Url;

/// A search's request, ready to send.
#[derive(Clone, PartialEq, Eq)]
pub struct Call {
    pub post: bool,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    /// The JSON body of a POST.
    pub body: Option<String>,
}

// The headers carry the API key: a debug print shows their names only.
impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call")
            .field("post", &self.post)
            .field("url", &self.url)
            .field(
                "headers",
                &self.headers.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            )
            .field("body", &self.body.as_ref().map(|b| b.len()))
            .finish()
    }
}

/// The answer is read to this size at most.
const MAX_ANSWER: u64 = 1024 * 1024;
/// Results are shortened to these, in characters.
const MAX_TITLE: usize = 200;
const MAX_SNIPPET: usize = 500;

const BRAVE: &str = "https://api.search.brave.com/res/v1/web/search";
const TAVILY: &str = "https://api.tavily.com/search";

/// The request for `query`, up to `count` results. `base` is a SearXNG
/// instance's address; `key` the API key of the others.
pub fn request(
    provider: Provider,
    key: &str,
    base: &str,
    query: &str,
    count: usize,
) -> Result<Call, WebError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(WebError::new("There is nothing to search for."));
    }
    let count = count.clamp(1, 20);
    match provider {
        Provider::Brave => {
            let mut url = Url::parse(BRAVE).expect("a fixed address");
            url.query_pairs_mut()
                .append_pair("q", query)
                .append_pair("count", &count.to_string())
                .append_pair("text_decorations", "false")
                .append_pair("result_filter", "web");
            Ok(Call {
                post: false,
                url: url.into(),
                headers: vec![
                    ("Accept", "application/json".into()),
                    ("X-Subscription-Token", key.trim().into()),
                ],
                body: None,
            })
        }
        Provider::Tavily => Ok(Call {
            post: true,
            url: TAVILY.into(),
            headers: vec![
                ("Accept", "application/json".into()),
                ("Content-Type", "application/json".into()),
                ("Authorization", format!("Bearer {}", key.trim())),
            ],
            body: Some(
                json!({"query": query, "max_results": count, "search_depth": "basic",
                       "include_answer": false})
                .to_string(),
            ),
        }),
        Provider::Searxng => {
            let mut url = instance(base)?;
            let path = format!("{}/search", url.path().trim_end_matches('/'));
            url.set_path(&path);
            url.query_pairs_mut()
                .append_pair("q", query)
                .append_pair("format", "json")
                .append_pair("categories", "general");
            Ok(Call {
                post: false,
                url: url.into(),
                headers: vec![("Accept", "application/json".into())],
                body: None,
            })
        }
    }
}

/// A SearXNG instance's address: http or https, no sign-in in it, no query.
pub fn instance(base: &str) -> Result<Url, WebError> {
    let base = base.trim();
    let url = Url::parse(base).map_err(|_| {
        WebError::new("The SearXNG address isn't a web address, such as http://localhost:8080.")
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(WebError::new(
            "The SearXNG address has to start with http:// or https://.",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(WebError::new(
            "Leave the user name and password out of the SearXNG address.",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(WebError::new("Leave the query out of the SearXNG address."));
    }
    Ok(url)
}

/// The results in an answer. Titles and snippets are plain text (the
/// services sometimes mark matches up), links are web addresses only, and
/// there are `count` at most.
pub fn parse(provider: Provider, body: &str, count: usize) -> Result<Vec<SearchResult>, WebError> {
    let value: Value = serde_json::from_str(body).map_err(|_| {
        WebError::new(format!(
            "{} sent an answer that isn't JSON.",
            provider.name()
        ))
    })?;
    let (list, snippet) = match provider {
        Provider::Brave => (value.pointer("/web/results"), "description"),
        Provider::Tavily | Provider::Searxng => (value.get("results"), "content"),
    };
    let list = match list {
        Some(Value::Array(list)) => list.as_slice(),
        // A search that found nothing has no list at all.
        None => &[],
        Some(_) => {
            return Err(WebError::new(format!(
                "{} sent results in a shape Gates doesn't know.",
                provider.name()
            )));
        }
    };
    let mut out: Vec<SearchResult> = Vec::new();
    for entry in list {
        let text = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("");
        let Some(url) = web_address(text("url")) else {
            continue;
        };
        if out.iter().any(|r| r.url == url) {
            continue;
        }
        let mut title = clean(text("title"), MAX_TITLE);
        if title.is_empty() {
            title = url.clone();
        }
        out.push(SearchResult {
            title,
            url,
            snippet: clean(text(snippet), MAX_SNIPPET),
        });
        if out.len() >= count {
            break;
        }
    }
    Ok(out)
}

/// `url` as an http or https address; None for anything else.
fn web_address(url: &str) -> Option<String> {
    let url = Url::parse(url.trim()).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then(|| url.into())
}

/// Markup, entities and odd characters out, spaces collapsed, cut to `max`
/// characters.
fn clean(text: &str, max: usize) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                plain.push(' ');
            }
            _ if in_tag => {}
            c => plain.push(c),
        }
    }
    let plain = html::decode(&plain);
    let line: String = plain
        .chars()
        .filter(|c| c.is_whitespace() || !super::is_invisible(*c))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Sends `call` and gives the body of a good answer. Errors say what to do.
pub fn send(agent: &ureq::Agent, provider: Provider, call: &Call) -> Result<String, WebError> {
    let name = provider.name();
    let result = if call.post {
        let mut request = agent.post(&call.url);
        for (key, value) in &call.headers {
            request = request.header(*key, value);
        }
        request.send(call.body.as_deref().unwrap_or("{}"))
    } else {
        let mut request = agent.get(&call.url);
        for (key, value) in &call.headers {
            request = request.header(*key, value);
        }
        request.call()
    };
    let response = result.map_err(|e| {
        // ureq's messages carry the address, which has no secret in it.
        WebError::new(format!("Couldn't reach {name}: {e}."))
    })?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(WebError::new(match status {
            401 | 403 if provider == Provider::Searxng => {
                "The SearXNG instance refused the search. Its settings.yml has to list json under search.formats."
                    .to_string()
            }
            401 | 403 => format!("{name} refused the API key ({status}). Check it in Settings."),
            402 => format!("{name} says the plan is used up (402)."),
            429 => format!("{name} says there have been too many searches (429). Wait a moment."),
            _ => format!("{name} answered {status}."),
        }));
    }
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_ANSWER)
        .read_to_string(&mut text)
        .map_err(|e| WebError::new(format!("The answer from {name} was cut off: {e}.")))?;
    Ok(text)
}

/// The client a search service is reached with: bounded in time, no proxy
/// (the key would pass through it), and the answer's status is the caller's
/// to read. A service with a key is only reached over https and never
/// follows a redirect, which would hand the key to wherever it points; a
/// SearXNG instance (no key, often plain http on the user's own network)
/// may redirect a few times.
pub fn agent(provider: Provider) -> ureq::Agent {
    ureq::Agent::new_with_config(client_config(provider.needs_key()))
}

/// The client's configuration (`agent`); `keyed` for a service with a key.
fn client_config(keyed: bool) -> ureq::config::Config {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(8)))
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .https_only(keyed)
        .max_redirects(if keyed { 0 } else { 3 })
        .proxy(None)
        .user_agent("telamon-gates")
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Answers as the services send them, written by hand.
    const BRAVE_ANSWER: &str = r#"{
      "type": "search",
      "query": {"original": "rust ownership"},
      "web": {"type": "search", "results": [
        {"title": "Understanding <strong>Ownership</strong> - The Rust Book",
         "url": "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html",
         "description": "Ownership is Rust&#x27;s most unique feature &amp; has deep implications.",
         "age": "2 weeks ago", "extra_snippets": ["more"]},
        {"title": "", "url": "https://example.org/untitled", "description": ""},
        {"title": "A script", "url": "javascript:alert(1)", "description": "no"},
        {"title": "Duplicate", "url": "https://example.org/untitled", "description": "again"}
      ]}
    }"#;

    const TAVILY_ANSWER: &str = r#"{
      "query": "rust ownership", "follow_up_questions": null, "answer": null,
      "images": [], "response_time": 0.9,
      "results": [
        {"title": "Ownership in Rust", "url": "https://example.net/ownership",
         "content": "Each value has one owner.\nWhen the owner goes out of scope the value is dropped.",
         "score": 0.91, "raw_content": null},
        {"title": "Borrowing", "url": "http://example.net/borrowing", "content": "References.", "score": 0.5}
      ]
    }"#;

    const SEARX_ANSWER: &str = r#"{
      "query": "rust ownership", "number_of_results": 0,
      "results": [
        {"url": "https://example.com/a", "title": "Ownership", "content": "Moves and copies.",
         "engine": "duckduckgo", "engines": ["duckduckgo"], "score": 1.0, "category": "general"},
        {"url": "ftp://example.com/b", "title": "Not the web", "content": "x", "engine": "x"},
        {"url": "https://example.com/c", "title": "No snippet", "engine": "bing"}
      ],
      "answers": [], "suggestions": ["rust borrow"], "unresponsive_engines": []
    }"#;

    #[test]
    fn brave_answers_are_read() {
        let got = parse(Provider::Brave, BRAVE_ANSWER, 8).unwrap();
        assert_eq!(
            got.len(),
            2,
            "the script link and the duplicate are left out"
        );
        assert_eq!(got[0].title, "Understanding Ownership - The Rust Book");
        assert_eq!(
            got[0].url,
            "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html"
        );
        assert_eq!(
            got[0].snippet,
            "Ownership is Rust's most unique feature & has deep implications."
        );
        // No title: the address stands in.
        assert_eq!(got[1].title, "https://example.org/untitled");
        assert_eq!(parse(Provider::Brave, BRAVE_ANSWER, 1).unwrap().len(), 1);
    }

    #[test]
    fn tavily_and_searxng_answers_are_read() {
        let got = parse(Provider::Tavily, TAVILY_ANSWER, 8).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0].snippet,
            "Each value has one owner. When the owner goes out of scope the value is dropped."
        );
        assert_eq!(got[1].url, "http://example.net/borrowing");
        let got = parse(Provider::Searxng, SEARX_ANSWER, 8).unwrap();
        assert_eq!(
            got.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
            vec!["https://example.com/a", "https://example.com/c"]
        );
        assert_eq!(got[1].snippet, "");
    }

    #[test]
    fn nothing_found_and_nonsense() {
        assert!(
            parse(Provider::Brave, r#"{"type": "search", "query": {}}"#, 5)
                .unwrap()
                .is_empty()
        );
        assert!(
            parse(Provider::Searxng, r#"{"results": []}"#, 5)
                .unwrap()
                .is_empty()
        );
        let e = parse(Provider::Tavily, "<html>Bad gateway</html>", 5).unwrap_err();
        assert!(e.to_string().contains("isn't JSON"), "{e}");
        let e = parse(Provider::Brave, r#"{"web": {"results": 5}}"#, 5).unwrap_err();
        assert!(e.to_string().contains("shape"), "{e}");
    }

    #[test]
    fn long_text_is_cut() {
        let body = json!({"results": [{"url": "https://example.com/", "title": "t".repeat(500),
                                       "content": "w ".repeat(600)}]})
        .to_string();
        let got = parse(Provider::Tavily, &body, 3).unwrap();
        assert_eq!(got[0].title.chars().count(), MAX_TITLE);
        assert!(got[0].title.ends_with('…'));
        assert!(got[0].snippet.chars().count() <= MAX_SNIPPET);
    }

    #[test]
    fn requests_keep_the_key_in_a_header() {
        let key = "test-key-not-real";
        let brave = request(Provider::Brave, key, "", "rust & ownership?", 99).unwrap();
        assert!(!brave.post && brave.body.is_none());
        let url = Url::parse(&brave.url).unwrap();
        assert_eq!(url.host_str(), Some("api.search.brave.com"));
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(pairs.contains(&("q".into(), "rust & ownership?".into())));
        assert!(pairs.contains(&("count".into(), "20".into())), "at most 20");
        assert!(
            brave
                .headers
                .contains(&("X-Subscription-Token", key.to_string()))
        );

        let tavily = request(Provider::Tavily, key, "", "q", 5).unwrap();
        assert!(tavily.post);
        assert_eq!(tavily.url, "https://api.tavily.com/search");
        assert!(
            tavily
                .headers
                .contains(&("Authorization", format!("Bearer {key}")))
        );
        let body: Value = serde_json::from_str(tavily.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["query"], "q");
        assert_eq!(body["max_results"], 5);

        let searx = request(
            Provider::Searxng,
            "",
            "http://localhost:8080/searx/",
            "a b",
            5,
        )
        .unwrap();
        assert_eq!(
            searx.url,
            "http://localhost:8080/searx/search?q=a+b&format=json&categories=general"
        );
        // No key is in any address.
        for call in [&brave, &tavily, &searx] {
            assert!(!call.url.contains(key));
        }
        assert!(request(Provider::Brave, key, "", "   ", 5).is_err());
    }

    #[test]
    fn a_printed_call_never_shows_the_key() {
        let key = "test-key-not-real";
        for provider in [Provider::Brave, Provider::Tavily] {
            let call = request(provider, key, "", "q", 3).unwrap();
            let shown = format!("{call:?}");
            assert!(!shown.contains(key), "{shown}");
            assert!(shown.contains("headers"));
        }
    }

    #[test]
    fn keyed_services_are_https_only_and_never_redirected() {
        for provider in [Provider::Brave, Provider::Tavily] {
            let config = client_config(provider.needs_key());
            assert!(config.https_only(), "{provider:?}");
            assert_eq!(config.max_redirects(), 0, "{provider:?}");
        }
        // SearXNG has no key to leak, and is often plain http, here or on
        // the user's network.
        let searx = client_config(Provider::Searxng.needs_key());
        assert!(!searx.https_only());
        assert!(searx.max_redirects() > 0);
    }

    #[test]
    fn an_instance_address_is_checked() {
        assert!(instance("http://localhost:8080").is_ok());
        assert!(instance("https://searx.example.org/").is_ok());
        for bad in [
            "",
            "searx.example.org",
            "ftp://x.org",
            "http://user:pass@x.org",
            "http://x.org/?q=1",
            "file:///etc/passwd",
        ] {
            assert!(instance(bad).is_err(), "{bad}");
        }
    }
}
