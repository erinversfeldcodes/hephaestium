//! A stand-in for a control plane's executor link, for tests: a small HTTP server on this
//! machine that keeps every byte each request sent and answers each as a test says. It is not
//! the control plane, and what it answers is only what a test wrote; the real one was run with
//! the worker by hand.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One request, as it arrived.
#[derive(Clone, Debug)]
pub struct Request {
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
    /// Every byte of it: the request line, the headers and the body.
    pub raw: Vec<u8>,
    pub at: Instant,
}

/// How a test answers a request.
pub enum Answer {
    Json(u16, String),
    NoContent,
    /// The request is read and the connection is closed without an answer, as a network that
    /// drops it does.
    Drop,
}

type Handler = Box<dyn FnMut(&Request) -> Answer + Send>;

pub struct Server {
    pub address: String,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    /// Listen on a free port of this machine and answer every request with `handler`.
    pub fn start(handler: impl FnMut(&Request) -> Answer + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = format!("http://{}", listener.local_addr().expect("an address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Mutex<Handler>> = Arc::new(Mutex::new(Box::new(handler)));
        let kept = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (seen, handler) = (Arc::clone(&kept), Arc::clone(&handler));
                std::thread::spawn(move || serve(stream, &seen, &handler));
            }
        });
        Self { address, seen }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().expect("the record").clone()
    }

    /// The requests to one route.
    pub fn to(&self, route: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|request| request.path == route)
            .collect()
    }

    /// Every byte every request sent.
    pub fn traffic(&self) -> Vec<u8> {
        self.requests()
            .into_iter()
            .flat_map(|request| request.raw)
            .collect()
    }

    /// Wait for what a test is waiting for to be so, for a while.
    pub fn wait_for(&self, what: &str, condition: impl Fn(&Self) -> bool) {
        let until = Instant::now() + Duration::from_secs(30);
        while !condition(self) {
            assert!(Instant::now() < until, "waited for {what}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn serve(stream: TcpStream, seen: &Mutex<Vec<Request>>, handler: &Mutex<Handler>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    loop {
        let mut raw = Vec::new();
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        raw.extend_from_slice(line.as_bytes());
        let path = line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        let mut length = 0;
        let mut authorization = None;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            raw.extend_from_slice(header.as_bytes());
            if header == "\r\n" {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap_or(0),
                    "authorization" => authorization = Some(value.trim().to_owned()),
                    _ => {}
                }
            }
        }
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        raw.extend_from_slice(&body);
        let request = Request {
            path,
            authorization,
            body: String::from_utf8_lossy(&body).into_owned(),
            raw,
            at: Instant::now(),
        };
        seen.lock().expect("the record").push(request.clone());
        let answer = (handler.lock().expect("the handler"))(&request);
        let (status, text) = match answer {
            Answer::Drop => {
                let _ = writer.shutdown(Shutdown::Both);
                return;
            }
            Answer::NoContent => (204, String::new()),
            Answer::Json(status, text) => (status, text),
        };
        let reply = format!(
            "HTTP/1.1 {status} Stand-in\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{text}",
            text.len()
        );
        if writer.write_all(reply.as_bytes()).is_err() {
            return;
        }
    }
}
