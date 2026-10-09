//! One streaming POST over plain HTTP/1.1, to the model server (on this
//! computer or the local network; Settings allows `http://` only), that can
//! be stopped from another thread: Stop shuts the socket, the read ends at
//! once, and llama-server sees the connection close and drops the work,
//! also while it is still reading a long prompt. A server that sends
//! nothing for `SILENCE` counts as gone.
//!
//! ureq can't be stopped mid-read from outside, hence this.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// How long the server may say nothing (a long prompt on the processor
/// takes a while; a hung server never ends).
pub const SILENCE: Duration = Duration::from_secs(10 * 60);
/// The longest status line or header kept, and headers at most.
const MAX_LINE: usize = 16 * 1024;
const MAX_HEADERS: usize = 100;

/// An answer under way: its status, and its body line by line.
pub struct Response {
    pub status: u16,
    body: Box<dyn BufRead + Send>,
    /// The connection, to shut from another thread (`until_stopped`).
    socket: TcpStream,
}

impl Response {
    /// The next line of the body (without its line end); None at the end.
    pub fn line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        if self.body.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        while line.ends_with(['\n', '\r']) {
            line.pop();
        }
        Ok(Some(line))
    }

    /// Runs `read` (the reading of this answer) and shuts the connection
    /// the moment `cancel` turns true, whatever `read` is waiting on.
    pub fn until_stopped<T>(
        &mut self,
        cancel: &AtomicBool,
        read: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let Ok(socket) = self.socket.try_clone() else {
            return read(self);
        };
        let (done, finished) = mpsc::channel::<()>();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                loop {
                    match finished.recv_timeout(Duration::from_millis(50)) {
                        Err(mpsc::RecvTimeoutError::Timeout) if !cancel.load(Ordering::Relaxed) => {
                        }
                        _ => break,
                    }
                }
                if cancel.load(Ordering::Relaxed) {
                    let _ = socket.shutdown(Shutdown::Both);
                }
            });
            let out = read(self);
            drop(done);
            out
        })
    }

    /// The whole body (an error's), up to 64 KiB.
    pub fn text(mut self) -> String {
        let mut out = String::new();
        let _ = (&mut self.body).take(64 * 1024).read_to_string(&mut out);
        out
    }
}

/// POSTs `body` (JSON) to `url` (`http://host:port/path`) with the headers
/// given, and gives the answer once its headers are in.
pub fn post(url: &str, headers: &[(&str, &str)], body: &str) -> io::Result<Response> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "only http:// addresses"))?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() || host.contains(['\r', '\n', ' ']) || path.contains(['\r', '\n', ' ']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an address",
        ));
    }
    let addr = host
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))?;
    let mut socket = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
    socket.set_read_timeout(Some(SILENCE))?;
    socket.set_write_timeout(Some(Duration::from_secs(60)))?;
    socket.set_nodelay(true)?;

    let stopper = socket.try_clone()?;
    let mut head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        if k.contains(['\r', '\n', ':']) || v.contains(['\r', '\n']) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad header"));
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    socket.write_all(head.as_bytes())?;
    socket.write_all(body.as_bytes())?;
    socket.flush()?;

    let mut reader = BufReader::new(socket);
    let status_line = read_line(&mut reader)?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an HTTP answer"))?;
    let mut chunked = false;
    let mut length: Option<u64> = None;
    for _ in 0..MAX_HEADERS {
        let line = read_line(&mut reader)?;
        if line.is_empty() {
            let body: Box<dyn BufRead + Send> = if chunked {
                Box::new(BufReader::new(Chunked {
                    inner: reader,
                    left: 0,
                    done: false,
                }))
            } else if let Some(n) = length {
                Box::new(BufReader::new(reader.take(n)))
            } else {
                Box::new(reader)
            };
            return Ok(Response {
                status,
                body,
                socket: stopper,
            });
        }
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            } else if k == "content-length" {
                length = v.parse().ok();
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "too many headers",
    ))
}

/// One line of the head, bounded.
fn read_line(r: &mut impl BufRead) -> io::Result<String> {
    let mut buf = Vec::new();
    (&mut *r)
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut buf)?;
    if buf.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the server closed the connection",
        ));
    }
    let line = String::from_utf8_lossy(&buf);
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// A chunked body as plain bytes.
struct Chunked<R: BufRead> {
    inner: R,
    left: u64,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.done || out.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let size = read_line(&mut self.inner)?;
            let size = size.split(';').next().unwrap_or("").trim();
            self.left = u64::from_str_radix(size, 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let want = out.len().min(self.left as usize);
        let n = self.inner.read(&mut out[..want])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the reply was cut off",
            ));
        }
        self.left -= n as u64;
        if self.left == 0 {
            // The line end after each chunk.
            let _ = read_line(&mut self.inner)?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Instant;

    fn serve(answer: &'static [u8], hold: Duration) -> (String, std::thread::JoinHandle<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(answer);
            // Then wait: does the client close on us?
            s.set_read_timeout(Some(hold)).unwrap();
            let started = Instant::now();
            // The rest of the request may still come; then the close.
            loop {
                match s.read(&mut buf) {
                    Ok(0) => return started.elapsed() < hold,
                    Ok(_) => {}
                    Err(_) => return false,
                }
            }
        });
        (url, handle)
    }

    #[test]
    fn chunked_lines() {
        let (url, _h) = serve(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\ndata: \r\na\r\na\n\ndata: b\r\n0\r\n\r\n",
            Duration::from_millis(100),
        );
        let mut r = post(&url, &[("Authorization", "Bearer k")], "{}").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.line().unwrap().as_deref(), Some("data: a"));
        assert_eq!(r.line().unwrap().as_deref(), Some(""));
        assert_eq!(r.line().unwrap().as_deref(), Some("data: b"));
        assert_eq!(r.line().unwrap(), None);
    }

    #[test]
    fn stop_closes_the_connection_while_the_server_is_silent() {
        // Headers, then nothing: the server is still reading a long prompt.
        let (url, server) = serve(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
            Duration::from_secs(5),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let mut r = post(&url, &[], "{}").unwrap();
        let stopper = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            stopper.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        // The read ends at once instead of waiting for the server.
        let got = r.until_stopped(&cancel, |r| r.line());
        assert!(started.elapsed() < Duration::from_secs(2), "{got:?}");
        assert!(
            server.join().unwrap(),
            "the server didn't see the connection close"
        );
    }

    #[test]
    fn errors_and_bad_addresses() {
        let (url, _h) = serve(
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 18\r\n\r\n{\"error\":\"nope\"}xx",
            Duration::from_millis(100),
        );
        let r = post(&url, &[], "{}").unwrap();
        assert_eq!(r.status, 400);
        assert_eq!(r.text(), "{\"error\":\"nope\"}xx");
        assert!(post("https://x/y", &[], "").is_err());
        assert!(post("http://a b/", &[], "").is_err());
        assert!(post("http://127.0.0.1:1/x", &[("Bad\r\nHeader", "v")], "").is_err());
    }
}
