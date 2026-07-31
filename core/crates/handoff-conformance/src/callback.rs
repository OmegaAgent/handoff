//! A local HTTP listener that captures signed callbacks the Server pushes (C-18).
//!
//! Hand-rolled rather than pulled from a framework: the suite must be trivially auditable by
//! someone deciding whether to trust a published conformance run, and "we accept a POST and record
//! it" should not require reading an async runtime to verify.
//!
//! It answers with a configurable status. `200` is the interesting one: §15.4 says a `2xx` marks a
//! callback *dispatched* and MUST NOT consume the signal, so a Server that stops redelivering on a
//! `200` has broken the effectively-once hinge and C-18 must catch it.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// One callback the Server sent us.
#[derive(Debug, Clone)]
pub struct Captured {
    /// Request target, as sent.
    pub target: String,
    /// Headers, lowercased keys.
    pub headers: BTreeMap<String, String>,
    /// Raw body, byte-for-byte, because a signature is over bytes and re-serializing changes them.
    pub body: String,
    /// When it arrived, as seconds since the epoch.
    pub at: u64,
}

impl Captured {
    /// The body parsed as JSON, or null when it is not JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// A running receiver.
pub struct Receiver {
    /// URL the Server should be told to call.
    pub url: String,
    captured: Arc<Mutex<Vec<Captured>>>,
    shutdown: Arc<Mutex<bool>>,
}

impl Receiver {
    /// Bind a listener and start accepting. `advertise` overrides the host and port written into
    /// the URL handed to the Server, for deployments that reach the runner through a tunnel.
    pub fn start(bind: &str, advertise: Option<&str>, respond_status: u16) -> Result<Self, String> {
        let listener = TcpListener::bind(bind)
            .map_err(|e| format!("cannot bind the callback receiver to {bind}: {e}"))?;
        let local = listener
            .local_addr()
            .map_err(|e| format!("cannot read the receiver's address: {e}"))?;
        let authority = advertise
            .map(str::to_string)
            .unwrap_or_else(|| local.to_string());
        let url = format!("http://{authority}/handoff-callback");

        let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(Mutex::new(false));

        listener
            .set_nonblocking(true)
            .map_err(|e| format!("cannot set the receiver non-blocking: {e}"))?;

        let sink = Arc::clone(&captured);
        let stop = Arc::clone(&shutdown);
        std::thread::spawn(move || loop {
            if *stop.lock().expect("receiver shutdown flag poisoned") {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let sink = Arc::clone(&sink);
                    std::thread::spawn(move || {
                        let _ = serve(stream, respond_status, &sink);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(_) => return,
            }
        });

        Ok(Self {
            url,
            captured,
            shutdown,
        })
    }

    /// Everything captured so far, oldest first.
    pub fn captured(&self) -> Vec<Captured> {
        self.captured.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Block until at least `n` callbacks have arrived, or the deadline passes.
    pub fn wait_for(&self, n: usize, timeout: std::time::Duration) -> usize {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let count = self.captured().len();
            if count >= n || std::time::Instant::now() >= deadline {
                return count;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Ok(mut stop) = self.shutdown.lock() {
            *stop = true;
        }
    }
}

fn serve(
    mut stream: std::net::TcpStream,
    respond_status: u16,
    sink: &Arc<Mutex<Vec<Captured>>>,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    let peer = stream.try_clone()?;
    let mut reader = BufReader::new(peer);

    let mut start_line = String::new();
    reader.read_line(&mut start_line)?;
    let target = start_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body)?;
    }

    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Ok(mut g) = sink.lock() {
        g.push(Captured {
            target,
            headers,
            body: String::from_utf8_lossy(&body).to_string(),
            at,
        });
    }

    let reason = match respond_status {
        200 => "OK",
        202 => "Accepted",
        500 => "Internal Server Error",
        _ => "Unspecified",
    };
    let response = format!(
        "HTTP/1.1 {respond_status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
