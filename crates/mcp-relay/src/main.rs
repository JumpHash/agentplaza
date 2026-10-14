//! `mcp_relay`: the pipe between an MCP container and apid's egress proxy (docs/design/mcp.md, "Running a
//! server"). It listens on `APZ_RELAY_LISTEN`. For each connection it dials `APZ_RELAY_UPSTREAM`, sends the
//! session token from `APZ_RELAY_TOKEN` and a PROXY protocol v1 line with the connection's source address, then
//! copies bytes both ways. It has no policy of its own: apid decides every request.
//!
//! The binary is built std-only, for the musl target, so it runs in alpine. It prints `ready` once it listens, and
//! exits when its stdin closes (apid holds that pipe, so the relay ends with apid).

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream};

/// The first two lines a relay connection starts with: the token, then the PROXY line.
fn hello(source: SocketAddrV4, listen: SocketAddrV4, token: &str) -> String {
    format!(
        "APZ-RELAY-TOKEN {token}\r\nPROXY TCP4 {} {} {} {}\r\n",
        source.ip(),
        listen.ip(),
        source.port(),
        listen.port()
    )
}

fn relay_one(client: TcpStream, upstream: &str, token: &str) -> io::Result<()> {
    let SocketAddr::V4(source) = client.peer_addr()? else {
        return Err(io::Error::other("only IPv4 connections are relayed"));
    };
    let SocketAddr::V4(listen) = client.local_addr()? else {
        return Err(io::Error::other("only IPv4 connections are relayed"));
    };
    let mut server = TcpStream::connect(upstream)?;
    server.write_all(hello(source, listen, token).as_bytes())?;
    pump(client, server);
    Ok(())
}

/// Copies both ways until either side closes.
fn pump(mut client: TcpStream, mut server: TcpStream) {
    let Ok(mut server_write) = server.try_clone() else {
        return;
    };
    let Ok(mut client_read) = client.try_clone() else {
        return;
    };
    let forward = std::thread::spawn(move || {
        let _ = io::copy(&mut client_read, &mut server_write);
        let _ = server_write.shutdown(Shutdown::Write);
    });
    let _ = io::copy(&mut server, &mut client);
    let _ = client.shutdown(Shutdown::Both);
    let _ = server.shutdown(Shutdown::Both);
    let _ = forward.join();
}

/// Relays every connection the listener accepts, one thread each.
pub fn serve(listener: TcpListener, upstream: String, token: String) {
    for client in listener.incoming().flatten() {
        let upstream = upstream.clone();
        let token = token.clone();
        std::thread::spawn(move || {
            let _ = relay_one(client, &upstream, &token);
        });
    }
}

fn required(name: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => value,
        _ => {
            eprintln!("mcp_relay: {name} is not set");
            std::process::exit(2);
        }
    }
}

fn main() {
    let listen = required("APZ_RELAY_LISTEN");
    let upstream = required("APZ_RELAY_UPSTREAM");
    let token = required("APZ_RELAY_TOKEN");
    let listener = match TcpListener::bind(&listen) {
        Ok(listener) => listener,
        Err(_) => {
            eprintln!("mcp_relay: cannot listen on {listen}");
            std::process::exit(2);
        }
    };
    println!("ready");
    let _ = io::stdout().flush();
    // apid holds stdin open for as long as it runs: when it closes, the relay ends.
    std::thread::spawn(|| {
        let mut buffer = [0u8; 64];
        let mut stdin = io::stdin();
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => std::process::exit(0),
                Ok(_) => {}
            }
        }
    });
    serve(listener, upstream, token);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::Ipv4Addr;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn the_hello_is_the_token_then_the_proxy_line_with_the_source() {
        let source = SocketAddrV4::new(Ipv4Addr::new(172, 29, 250, 9), 5000);
        let listen = SocketAddrV4::new(Ipv4Addr::new(172, 29, 250, 2), 3128);
        assert_eq!(
            hello(source, listen, "tok"),
            "APZ-RELAY-TOKEN tok\r\nPROXY TCP4 172.29.250.9 172.29.250.2 5000 3128\r\n"
        );
    }

    #[test]
    fn a_connection_is_relayed_with_the_token_and_the_source_and_bytes_both_ways() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let (seen_tx, seen_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = upstream.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut token_line = String::new();
            let mut proxy_line = String::new();
            let mut request = String::new();
            reader.read_line(&mut token_line).unwrap();
            reader.read_line(&mut proxy_line).unwrap();
            reader.read_line(&mut request).unwrap();
            let mut stream = reader.into_inner();
            stream.write_all(b"pong\n").unwrap();
            seen_tx.send((token_line, proxy_line, request)).unwrap();
        });
        let relay = TcpListener::bind("127.0.0.1:0").unwrap();
        let relay_addr = relay.local_addr().unwrap();
        let upstream_text = upstream_addr.to_string();
        std::thread::spawn(move || serve(relay, upstream_text, "tok".into()));
        let mut client = TcpStream::connect(relay_addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client.write_all(b"ping\n").unwrap();
        let mut reply = String::new();
        BufReader::new(client).read_line(&mut reply).unwrap();
        assert_eq!(reply, "pong\n");
        let (token_line, proxy_line, request) =
            seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(token_line, "APZ-RELAY-TOKEN tok\r\n");
        let fields: Vec<&str> = proxy_line.trim_end().split(' ').collect();
        assert_eq!(fields.len(), 6, "{proxy_line}");
        assert_eq!(&fields[..4], &["PROXY", "TCP4", "127.0.0.1", "127.0.0.1"]);
        assert_eq!(fields[5], relay_addr.port().to_string());
        assert!(fields[4].parse::<u16>().is_ok(), "{proxy_line}");
        assert_eq!(request, "ping\n");
    }
}
