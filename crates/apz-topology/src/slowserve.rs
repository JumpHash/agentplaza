//! A stand-in service for load tests (PLAN 22.8): an HTTP server on the loopback address that answers every request
//! with a fixed body after a delay the owner sets, so a test can seed a slowdown and see the load generator and the
//! comparison report it. It answers `Connection: close` and serves one request per connection.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

/// A running stand-in. It stops when dropped.
pub struct SlowServer {
    addr: SocketAddr,
    delay_ms: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl SlowServer {
    /// Starts a server on a free loopback port, answering after `delay_ms`.
    pub fn start(delay_ms: u64) -> std::io::Result<SlowServer> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let delay_ms = Arc::new(AtomicU64::new(delay_ms));
        let stop = Arc::new(AtomicBool::new(false));
        let (delay, halt) = (Arc::clone(&delay_ms), Arc::clone(&stop));
        thread::spawn(move || {
            for stream in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let delay = Arc::clone(&delay);
                thread::spawn(move || answer(stream, delay.load(Ordering::SeqCst)));
            }
        });
        Ok(SlowServer {
            addr,
            delay_ms,
            stop,
        })
    }

    /// Sets the delay of the requests that arrive from now on.
    pub fn set_delay(&self, delay_ms: u64) {
        self.delay_ms.store(delay_ms, Ordering::SeqCst);
    }

    /// The URL the server answers at.
    pub fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// The address the server listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for SlowServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
    }
}

fn answer(mut stream: TcpStream, delay_ms: u64) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    thread::sleep(Duration::from_millis(delay_ms));
    let body = b"ok";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_answers_after_the_delay_the_test_sets() {
        let server = SlowServer::start(0).unwrap();
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build(),
        );
        let fast = std::time::Instant::now();
        let response = agent.get(&server.url()).call().unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert!(fast.elapsed() < Duration::from_millis(900));
        server.set_delay(150);
        let slow = std::time::Instant::now();
        agent.get(&server.url()).call().unwrap();
        assert!(slow.elapsed() >= Duration::from_millis(150));
    }
}
