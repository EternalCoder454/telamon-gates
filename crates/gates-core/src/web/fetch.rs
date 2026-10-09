//! `fetch_page`: one web page as plain text, with the model never in charge
//! of where the request goes. A page it names may be anywhere on the
//! internet, so:
//!
//! - **https only**, no sign-in in the address, and every redirect is
//!   followed here, one hop at a time, each checked again (the client never
//!   follows one on its own);
//! - **public addresses only.** The name is resolved by a resolver that
//!   drops every private, loopback, link-local and other non-public address,
//!   and the connection goes to one of those it kept. The address checked is
//!   the address connected to, so a name that answers differently the second
//!   time (DNS rebinding) gets nothing. A page can't make Gates probe the
//!   local network, this computer or a cloud metadata service;
//! - **bounded**: 15 s in all, 5 redirects, 1.5 MB read, 20 KB of text kept.

use super::{Page, WebError, html};
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::{Host, Url};

/// The text of a page kept for the model, in bytes.
pub const MAX_TEXT: usize = 20 * 1024;

/// What the connection may reach. Only `STRICT` is used by the app; the
/// tests serve plain http on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub https_only: bool,
    pub allow_private: bool,
}

impl Policy {
    pub const STRICT: Policy = Policy {
        https_only: true,
        allow_private: false,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// For the whole fetch of one address, redirects not counted apart.
    pub timeout: Duration,
    pub connect: Duration,
    /// Bytes of the page read.
    pub body: usize,
    pub redirects: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            timeout: Duration::from_secs(15),
            connect: Duration::from_secs(6),
            body: 1536 * 1024,
            redirects: 5,
        }
    }
}

/// Said for a refused address, and recognised in the connection's error.
const PRIVATE: &str = "Gates only opens public web pages, not this computer or a local network.";

/// Whether `ip` is an address on the public internet.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        // 0.0.0.0/8, "this network".
        || a == 0
        // 100.64.0.0/10, carrier-grade NAT (also Tailscale).
        || (a == 100 && (64..128).contains(&b))
        // 192.0.0.0/24, IETF protocol assignments.
        || (a == 192 && b == 0 && c == 0)
        // 198.18.0.0/15, benchmarking.
        || (a == 198 && (b == 18 || b == 19))
        // 240.0.0.0/4, reserved.
        || a >= 240)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    // Addresses that carry an IPv4 address are judged by it.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let s = ip.segments();
    // 64:ff9b::/96, NAT64.
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        let [a, b] = s[6].to_be_bytes();
        let [c, d] = s[7].to_be_bytes();
        return public_v4(Ipv4Addr::new(a, b, c, d));
    }
    // 2002::/16, 6to4.
    if s[0] == 0x2002 {
        let [a, b] = s[1].to_be_bytes();
        let [c, d] = s[2].to_be_bytes();
        return public_v4(Ipv4Addr::new(a, b, c, d));
    }
    // ::a.b.c.d, deprecated IPv4-compatible.
    if s[..6] == [0; 6] {
        let [a, b] = s[6].to_be_bytes();
        let [c, d] = s[7].to_be_bytes();
        return public_v4(Ipv4Addr::new(a, b, c, d));
    }
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        // fec0::/10, deprecated site-local.
        || (s[0] & 0xffc0) == 0xfec0
        // 2001:db8::/32, documentation.
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        // 100::/64, discard-only.
        || (s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0))
}

/// The addresses of `all` that are public.
pub fn only_public(all: &[SocketAddr]) -> Vec<SocketAddr> {
    all.iter().copied().filter(|a| public_ip(a.ip())).collect()
}

/// Resolves like the default and keeps the public addresses.
#[derive(Debug)]
struct Guard {
    inner: DefaultResolver,
    allow_private: bool,
}

impl Resolver for Guard {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let all = self.inner.resolve(uri, config, timeout)?;
        if self.allow_private {
            return Ok(all);
        }
        let mut kept = self.inner.empty();
        for addr in only_public(&all) {
            kept.push(addr);
        }
        if kept.is_empty() {
            return Err(ureq::Error::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                PRIVATE,
            )));
        }
        Ok(kept)
    }
}

/// Opens web pages. Cheap to keep: it holds an HTTP client.
pub struct Fetcher {
    agent: ureq::Agent,
    policy: Policy,
    limits: Limits,
}

const ACCEPT: &str = "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.1";

impl Fetcher {
    pub fn new() -> Fetcher {
        Fetcher::with(Policy::STRICT, Limits::default())
    }

    /// Any policy: for tests that serve plain http here.
    pub fn with(policy: Policy, limits: Limits) -> Fetcher {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        let config = Config::builder()
            .https_only(policy.https_only)
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .timeout_connect(Some(limits.connect))
            .timeout_global(Some(limits.timeout))
            .http_status_as_error(false)
            // Redirects are followed here, each one checked.
            .max_redirects(0)
            // A proxy would resolve the name itself, past the check.
            .proxy(None)
            .max_response_header_size(32 * 1024)
            .user_agent("Mozilla/5.0 (compatible; TelamonGates; +https://github.com/EternalCoder454/telamon-gates)")
            .build();
        let resolver = Guard {
            inner: DefaultResolver::default(),
            allow_private: policy.allow_private,
        };
        Fetcher {
            agent: ureq::Agent::with_parts(config, DefaultConnector::default(), resolver),
            policy,
            limits,
        }
    }

    /// The address as it may be opened, or why not. Literal addresses are
    /// judged here (a clearer answer); names, by the resolver.
    pub fn check(&self, url: &Url) -> Result<(), WebError> {
        let https = url.scheme() == "https";
        if !(https || (url.scheme() == "http" && !self.policy.https_only)) {
            return Err(WebError::new(
                "Only https:// addresses can be opened (http:// is not safe).",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(WebError::new(
                "Addresses with a user name or password are not opened.",
            ));
        }
        let ip = match url.host() {
            None => return Err(WebError::new("That address has no website in it.")),
            Some(Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
            Some(Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
            Some(Host::Domain(_)) => None,
        };
        if let Some(ip) = ip
            && !self.policy.allow_private
            && !public_ip(ip)
        {
            return Err(WebError::new(PRIVATE));
        }
        Ok(())
    }

    /// The page at `start`, as text. Blocks (up to the time limit): call it
    /// from a worker.
    pub fn get(&self, start: &str, cancel: &AtomicBool) -> Result<Page, WebError> {
        let mut url = Url::parse(start.trim()).map_err(|_| {
            WebError::new(
                "That isn't a web address. Use a full one, such as https://example.com/page.",
            )
        })?;
        for _ in 0..=self.limits.redirects {
            if cancel.load(Ordering::Relaxed) {
                return Err(WebError::new("Stopped."));
            }
            self.check(&url)?;
            let response = self
                .agent
                .get(url.as_str())
                .header("Accept", ACCEPT)
                .call()
                .map_err(explain)?;
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| WebError::new("The page redirects nowhere."))?;
                url = url.join(location).map_err(|_| {
                    WebError::new("The page redirects to an address that isn't valid.")
                })?;
                continue;
            }
            if status != 200 {
                return Err(WebError::new(match status {
                    401 | 403 => format!("The page refuses to be opened ({status})."),
                    404 | 410 => format!("There is no such page ({status})."),
                    429 => "The website says there have been too many requests (429).".to_string(),
                    _ => format!("The website answered {status}."),
                }));
            }
            return self.read(url, response);
        }
        Err(WebError::new(format!(
            "The page redirects more than {} times.",
            self.limits.redirects
        )))
    }

    fn read(&self, url: Url, response: ureq::http::Response<ureq::Body>) -> Result<Page, WebError> {
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_ascii_lowercase();
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let kind = match mime.as_str() {
            "" | "text/html" | "application/xhtml+xml" => Kind::Html,
            m if m.starts_with("text/")
                || matches!(
                    m,
                    "application/json"
                        | "application/xml"
                        | "application/rss+xml"
                        | "application/atom+xml"
                ) =>
            {
                Kind::Text
            }
            other => {
                return Err(WebError::new(format!(
                    "That page is {other}, which isn't text. Only web pages and text can be read."
                )));
            }
        };
        let mut bytes = Vec::new();
        response
            .into_body()
            .into_reader()
            .take(self.limits.body as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| WebError::new(format!("The page stopped answering: {e}.")))?;
        let cut = bytes.len() > self.limits.body;
        bytes.truncate(self.limits.body);
        let charset = charset_of(&content_type, &bytes);
        let source = decode_body(&bytes, &charset);
        let (title, text) = match kind {
            Kind::Html => {
                let page = html::extract(&source, &url);
                (page.title, page.text)
            }
            Kind::Text => (String::new(), plain(&source)),
        };
        let (text, capped) = cap(&text, MAX_TEXT);
        let mut final_url = url;
        final_url.set_fragment(None);
        Ok(Page {
            url: final_url.into(),
            title,
            text,
            truncated: cut || capped,
        })
    }
}

impl Default for Fetcher {
    fn default() -> Fetcher {
        Fetcher::new()
    }
}

enum Kind {
    Html,
    Text,
}

/// What went wrong with a request, in a sentence.
fn explain(error: ureq::Error) -> WebError {
    match error {
        ureq::Error::Io(e)
            if e.kind() == io::ErrorKind::PermissionDenied && e.to_string() == PRIVATE =>
        {
            WebError::new(PRIVATE)
        }
        ureq::Error::Timeout(_) => WebError::new("The website took too long to answer."),
        ureq::Error::HostNotFound => WebError::new("There is no website by that name."),
        ureq::Error::RequireHttpsOnly(_) => WebError::new("Only https:// addresses can be opened."),
        other => WebError::new(format!("Couldn't open the page: {other}.")),
    }
}

/// The text encoding a page says it has: its header's, else its first
/// bytes' (`<meta charset>`); utf-8 when it says nothing.
fn charset_of(content_type: &str, bytes: &[u8]) -> String {
    let find = |text: &str| {
        let at = text.find("charset=")? + "charset=".len();
        let rest = text[at..].trim_start_matches(['"', '\'']);
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(rest.len());
        Some(rest[..end].to_ascii_lowercase())
    };
    find(content_type)
        .or_else(|| {
            let head =
                String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_ascii_lowercase();
            find(&head)
        })
        .unwrap_or_else(|| "utf-8".into())
}

/// `bytes` as text: Latin-1 and Windows-1252 as they are (the common
/// others), anything else as UTF-8, bad bytes replaced.
fn decode_body(bytes: &[u8], charset: &str) -> String {
    match charset {
        "iso-8859-1" | "latin1" | "windows-1252" | "cp1252" | "iso-8859-15" => {
            bytes.iter().map(|&b| char::from(b)).collect()
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Plain text, tidied: no invisible characters, lines trimmed, runs of empty
/// lines made one.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut empty = 0;
    for line in text.lines() {
        let line: String = line.chars().filter(|c| !super::is_invisible(*c)).collect();
        let line = line.trim_end();
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
        out.push_str(line);
    }
    out
}

/// `text` cut to `max` bytes at a line (or else a character) end, and
/// whether it was cut.
pub fn cap(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // A line end in the last fifth, so a sentence isn't cut in two.
    if let Some(nl) = text[..end].rfind('\n')
        && nl > max / 5 * 4
    {
        end = nl;
    }
    (text[..end].trim_end().to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Instant;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn private_and_special_addresses_are_not_public() {
        for private in [
            "127.0.0.1",
            "127.1.2.3",
            "10.0.0.5",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1",
            "100.127.255.255",
            "192.0.0.8",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "192.0.2.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "2002:a9fe:a9fe::1",
            "::127.0.0.1",
        ] {
            assert!(!public_ip(ip(private)), "{private} must be refused");
        }
        for public in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "2606:4700:4700::1111",
            "2a00:1450:4001::200e",
            "::ffff:8.8.8.8",
        ] {
            assert!(public_ip(ip(public)), "{public} is public");
        }
    }

    #[test]
    fn the_resolver_keeps_the_public_addresses_only() {
        let all: Vec<SocketAddr> = [
            "10.0.0.1:443",
            "93.184.216.34:443",
            "[::1]:443",
            "[2606:4700::1111]:443",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        let kept = only_public(&all);
        assert_eq!(kept, vec![all[1], all[3]]);
        assert!(only_public(&all[..1]).is_empty());
    }

    #[test]
    fn addresses_are_checked_before_any_request() {
        let strict = Fetcher::new();
        let stop = AtomicBool::new(false);
        for (address, wants) in [
            ("http://example.com/", "https://"),
            ("ftp://example.com/", "https://"),
            ("file:///etc/passwd", "https://"),
            ("https://user:pw@example.com/", "user name"),
            ("https://127.0.0.1/", "public web pages"),
            ("https://[::1]/", "public web pages"),
            (
                "https://169.254.169.254/latest/meta-data/",
                "public web pages",
            ),
            ("https://192.168.0.1:8443/admin", "public web pages"),
            ("https://[::ffff:10.0.0.1]/", "public web pages"),
            ("https://0x7f.1/", "public web pages"),
            ("https://2130706433/", "public web pages"),
            ("example", "web address"),
            ("", "web address"),
        ] {
            let e = strict.get(address, &stop).unwrap_err().to_string();
            assert!(e.contains(wants), "{address}: {e}");
        }
    }

    // ---- against a server on this computer (plain http, the lax policy)

    /// A server that answers each connection with the next response of
    /// `answers`, whole, then closes it. `stall` holds the connection
    /// instead, for a timeout.
    fn serve(answers: Vec<Vec<u8>>, stall: bool) -> (u16, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut socket, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf);
                if stall {
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
                let _ = socket.write_all(&answer);
            }
        });
        (port, handle)
    }

    fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn lax(limits: Limits) -> Fetcher {
        Fetcher::with(
            Policy {
                https_only: false,
                allow_private: true,
            },
            limits,
        )
    }

    fn quick() -> Limits {
        Limits {
            timeout: Duration::from_millis(800),
            connect: Duration::from_millis(500),
            ..Limits::default()
        }
    }

    #[test]
    fn a_page_becomes_text() {
        let body =
            b"<html><head><title>Hi</title></head><body><h1>Hello</h1><p>World \xe2\x9c\x93</p>\
                     <script>x()</script></body></html>";
        let (port, server) = serve(
            vec![response(
                "200 OK",
                "Content-Type: text/html; charset=utf-8\r\n",
                body,
            )],
            false,
        );
        let page = lax(quick())
            .get(
                &format!("http://127.0.0.1:{port}/a#frag"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(page.title, "Hi");
        assert_eq!(page.text, "# Hello\n\nWorld ✓");
        assert_eq!(page.url, format!("http://127.0.0.1:{port}/a"));
        assert!(!page.truncated);
        server.join().unwrap();
    }

    #[test]
    fn the_real_check_refuses_this_computer_by_name() {
        // Plain http allowed, private addresses not: "localhost" is a name,
        // so only the resolver can refuse it, and it must.
        let (port, server) = serve(vec![response("200 OK", "", b"secret")], false);
        let fetcher = Fetcher::with(
            Policy {
                https_only: false,
                allow_private: false,
            },
            quick(),
        );
        let e = fetcher
            .get(
                &format!("http://localhost:{port}/"),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(e.to_string().contains("public web pages"), "{e}");
        // Nothing connected: the server is still waiting.
        let _ = std::net::TcpStream::connect(("127.0.0.1", port));
        server.join().unwrap();
    }

    #[test]
    fn redirects_are_followed_one_by_one_up_to_a_limit() {
        let hop = |to: &str| response("302 Found", &format!("Location: {to}\r\n"), b"");
        let ok = response(
            "200 OK",
            "Content-Type: text/plain\r\n",
            b"arrived\n\n\n\nhere",
        );
        let (port, server) = serve(
            vec![hop("/b"), hop("http://127.0.0.1:1/c-is-elsewhere")],
            false,
        );
        // The second hop leaves for another port: nothing listens there.
        let e = lax(quick())
            .get(
                &format!("http://127.0.0.1:{port}/a"),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(e.to_string().contains("Couldn't open"), "{e}");
        server.join().unwrap();

        let (port, server) = serve(vec![hop("/b"), hop("/c"), ok], false);
        let page = lax(quick())
            .get(
                &format!("http://127.0.0.1:{port}/a"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(page.text, "arrived\n\nhere");
        assert_eq!(page.url, format!("http://127.0.0.1:{port}/c"));
        server.join().unwrap();

        // A loop ends at the limit.
        let (port, server) = serve((0..4).map(|_| hop("/again")).collect(), false);
        let limits = Limits {
            redirects: 3,
            ..quick()
        };
        let e = lax(limits)
            .get(
                &format!("http://127.0.0.1:{port}/again"),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(e.to_string().contains("more than 3 times"), "{e}");
        server.join().unwrap();
    }

    #[test]
    fn a_redirect_to_a_private_or_plain_address_is_refused() {
        // The strict check on the second hop: first a lax server, then the
        // location is judged by `check`.
        let strict = Fetcher::new();
        let base = Url::parse("https://example.com/start").unwrap();
        for location in [
            "http://example.com/plain",
            "https://127.0.0.1/admin",
            "//192.168.0.1/router",
            "https://[::1]:9000/",
            "https://u:p@example.com/",
        ] {
            let next = base.join(location).unwrap();
            assert!(strict.check(&next).is_err(), "{location}");
        }
        assert!(strict.check(&base.join("/next?a=1").unwrap()).is_ok());
    }

    #[test]
    fn big_pages_are_cut_in_what_is_read_and_what_is_kept() {
        let mut body = String::from("<html><body>");
        let mut n = 0;
        while body.len() < 4 * 1024 * 1024 {
            body.push_str(&format!(
                "<p>Paragraph number {n} with some words in it.</p>"
            ));
            n += 1;
        }
        let (port, server) = serve(
            vec![response(
                "200 OK",
                "Content-Type: text/html\r\n",
                body.as_bytes(),
            )],
            false,
        );
        let limits = Limits {
            timeout: Duration::from_secs(10),
            ..Limits::default()
        };
        let page = lax(limits)
            .get(
                &format!("http://127.0.0.1:{port}/"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(page.truncated);
        assert!(page.text.len() <= MAX_TEXT, "{} bytes", page.text.len());
        assert!(page.text.len() > MAX_TEXT / 2);
        assert!(page.text.starts_with("Paragraph number 0 "));
        let _ = server.join();
    }

    #[test]
    fn a_slow_site_times_out() {
        let (port, server) = serve(vec![Vec::new()], true);
        let started = Instant::now();
        let e = lax(quick())
            .get(
                &format!("http://127.0.0.1:{port}/"),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert!(
            e.to_string().contains("took too long") || e.to_string().contains("stopped"),
            "{e}"
        );
        let _ = server.join();
    }

    #[test]
    fn errors_and_other_kinds_of_page() {
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (response("404 Not Found", "", b"no"), "no such page"),
            (response("500 Oops", "", b"no"), "answered 500"),
            (
                response("200 OK", "Content-Type: image/png\r\n", b"\x89PNG"),
                "image/png",
            ),
            (
                response("200 OK", "Content-Type: application/pdf\r\n", b"%PDF"),
                "application/pdf",
            ),
            (response("302 Found", "", b""), "redirects nowhere"),
        ];
        for (answer, wants) in cases {
            let (port, server) = serve(vec![answer], false);
            let e = lax(quick())
                .get(
                    &format!("http://127.0.0.1:{port}/"),
                    &AtomicBool::new(false),
                )
                .unwrap_err();
            assert!(e.to_string().contains(wants), "{wants}: {e}");
            server.join().unwrap();
        }
        // Plain text and JSON are read as they are, in their own encoding.
        let (port, server) = serve(
            vec![response(
                "200 OK",
                "Content-Type: text/plain; charset=iso-8859-1\r\n",
                b"caf\xe9  \n\n\n\ntea",
            )],
            false,
        );
        let page = lax(quick())
            .get(
                &format!("http://127.0.0.1:{port}/"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(page.text, "café\n\ntea");
        server.join().unwrap();
    }

    #[test]
    fn stop_ends_a_fetch_before_it_starts() {
        let stop = Arc::new(AtomicBool::new(true));
        let e = lax(quick()).get("http://127.0.0.1:9/", &stop).unwrap_err();
        assert_eq!(e.to_string(), "Stopped.");
    }

    #[test]
    fn text_is_cut_at_a_line() {
        let text = format!("{}\n{}", "a".repeat(90), "b".repeat(50));
        let (cut, was) = cap(&text, 100);
        assert!(was);
        assert_eq!(cut, "a".repeat(90));
        let (same, was) = cap("short", 100);
        assert!(!was && same == "short");
        // Never inside a character.
        let (cut, _) = cap(&"é".repeat(100), 101);
        assert_eq!(cut.chars().count(), 50);
    }
}
