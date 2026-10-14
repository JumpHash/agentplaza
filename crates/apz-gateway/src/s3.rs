//! A minimal S3 client (path-style, SigV4) for the plaza's object store (Garage):
//! put, get and delete one object. Bodies are held in memory.

use std::io::Read;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::GatewayError;
use crate::http::{Http, Req, reader};
use crate::sigv4::{self, Credentials, uri_encode};

/// Whole-call timeout for one S3 request.
const TIMEOUT: Duration = Duration::from_secs(120);
/// Retries of connection errors and 408/409/429/5xx.
const RETRIES: u32 = 2;
const RETRY_BACKOFF: Duration = Duration::from_millis(250);
/// Bytes of an unexpected response body kept in an error.
const ERROR_BODY_BYTES: usize = 300;

/// One bucket on an S3-compatible endpoint.
pub struct S3 {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

impl S3 {
    /// Stores `body` at `key` with `content_type`.
    pub fn put(&self, key: &str, body: &[u8], content_type: &str) -> Result<(), String> {
        self.call("PUT", key, body.to_vec(), Some(content_type))
            .map(|_| ())
            .map_err(fail)
    }

    /// The object at `key`.
    pub fn get(&self, key: &str) -> Result<Vec<u8>, String> {
        self.call("GET", key, Vec::new(), None)
            .map_err(|e| match e {
                GatewayError::Http { status: 404, .. } => "not found".to_string(),
                other => fail(other),
            })
    }

    /// Removes the object at `key` (absent is not an error).
    pub fn delete(&self, key: &str) -> Result<(), String> {
        match self.call("DELETE", key, Vec::new(), None) {
            Ok(_) | Err(GatewayError::Http { status: 404, .. }) => Ok(()),
            Err(e) => Err(fail(e)),
        }
    }

    /// `{endpoint}/{bucket}/{key}`, each key segment URI-encoded.
    fn url(&self, key: &str) -> String {
        let segments: Vec<String> = key.split('/').map(uri_encode).collect();
        format!(
            "{}/{}/{}",
            self.endpoint.trim_end_matches('/'),
            uri_encode(&self.bucket),
            segments.join("/")
        )
    }

    /// The request for `verb` on `key`, signed with the bucket's credentials.
    fn signed(
        &self,
        verb: &'static str,
        key: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Req {
        let mut req = Req::raw(verb, self.url(key), body);
        if let Some(content_type) = content_type {
            req.set_header("content-type", content_type);
        }
        let hash = payload_hash(&req.body);
        req.set_header("x-amz-content-sha256", &hash);
        let creds = Credentials {
            access_key: self.access_key.clone(),
            secret_key: self.secret_key.clone(),
            session_token: None,
        };
        sigv4::sign(&mut req, &creds, &self.region, "s3", SystemTime::now());
        req
    }

    /// Sends one request and returns the response body.
    fn call(
        &self,
        verb: &'static str,
        key: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<Vec<u8>, GatewayError> {
        let req = self.signed(verb, key, body, content_type);
        let resp = Http::with_timeout(RETRIES, RETRY_BACKOFF, TIMEOUT).send(&req)?;
        let mut out = Vec::new();
        reader(resp)
            .read_to_end(&mut out)
            .map_err(|e| GatewayError::Transport(format!("s3 read: {e}")))?;
        Ok(out)
    }
}

fn payload_hash(body: &[u8]) -> String {
    Sha256::digest(body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A failed request as the message the caller sees.
fn fail(e: GatewayError) -> String {
    match e {
        GatewayError::Http { status, body, .. } => {
            let kept = &body.as_bytes()[..body.len().min(ERROR_BODY_BYTES)];
            format!("s3 {status}: {}", String::from_utf8_lossy(kept))
        }
        GatewayError::Transport(message) => message,
        _ => "s3 request failed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn client(endpoint: &str) -> S3 {
        S3 {
            endpoint: endpoint.into(),
            bucket: "plaza".into(),
            region: "garage".into(),
            access_key: "GKTEST".into(),
            secret_key: "sekrit".into(),
        }
    }

    #[test]
    fn s3_url_encodes_each_key_segment() {
        let s3 = client("http://127.0.0.1:3900/");
        assert_eq!(
            s3.url("models/a b+c.json"),
            "http://127.0.0.1:3900/plaza/models/a%20b%2Bc.json"
        );
        assert_eq!(s3.url("x"), "http://127.0.0.1:3900/plaza/x");
    }

    #[test]
    fn s3_signed_put_carries_sigv4_headers() {
        let req =
            client("http://127.0.0.1:3900").signed("PUT", "k", b"hi".to_vec(), Some("text/plain"));
        assert_eq!(req.method(), "PUT");
        let amz = req.header_value("x-amz-date").unwrap().to_string();
        assert_eq!(amz.len(), 16);
        assert_eq!(
            req.header_value("x-amz-content-sha256"),
            Some(payload_hash(b"hi").as_str())
        );
        let auth = req.header_value("authorization").unwrap();
        let prefix = format!(
            "AWS4-HMAC-SHA256 Credential=GKTEST/{}/garage/s3/aws4_request, ",
            &amz[..8]
        );
        assert!(auth.starts_with(&prefix), "{auth}");
    }

    /// Reads one request (head, then the declared body) off `conn`.
    fn read_more(conn: &mut TcpStream, buf: &mut Vec<u8>) {
        let mut chunk = [0u8; 4096];
        let n = conn.read(&mut chunk).unwrap();
        assert!(n > 0, "client closed the connection early");
        buf.extend_from_slice(&chunk[..n]);
    }

    /// A stand-in S3: answers one request per canned reply on its own
    /// connection, and returns the request heads it saw.
    fn fake_s3(replies: Vec<&'static str>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut heads = Vec::new();
            for reply in replies {
                let (mut conn, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    read_more(&mut conn, &mut buf);
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                let length = head
                    .lines()
                    .find_map(|l| {
                        let l = l.to_ascii_lowercase();
                        l.strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while buf.len() < head_end + length {
                    read_more(&mut conn, &mut buf);
                }
                conn.write_all(reply.as_bytes()).unwrap();
                heads.push(head);
            }
            heads
        });
        (endpoint, handle)
    }

    #[test]
    fn s3_put_get_delete_round_trip_over_http() {
        let (endpoint, server) = fake_s3(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n",
        ]);
        let s3 = client(&endpoint);
        s3.put("a b", b"hello", "text/plain").unwrap();
        assert_eq!(s3.get("a b").unwrap(), b"hello");
        assert_eq!(s3.get("gone").unwrap_err(), "not found");
        s3.delete("gone").unwrap();

        let heads = server.join().unwrap();
        assert!(
            heads[0].starts_with("PUT /plaza/a%20b HTTP/1.1\r\n"),
            "{}",
            heads[0]
        );
        assert!(
            heads[1].starts_with("GET /plaza/a%20b HTTP/1.1\r\n"),
            "{}",
            heads[1]
        );
        assert!(
            heads[3].starts_with("DELETE /plaza/gone HTTP/1.1\r\n"),
            "{}",
            heads[3]
        );
        assert!(
            heads[0]
                .to_ascii_lowercase()
                .contains("authorization: aws4-hmac-sha256 credential=gktest/")
        );
    }
}
