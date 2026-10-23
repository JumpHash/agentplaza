//! apz-topo-echo: a stand-in service for the topology tests (PLAN 22.6), run by `apz_topology::run` in place of an
//! artifact.
//!
//! It listens on each port of `APZ_TOPO_PORTS` at `APZ_TOPO_BIND`. Without `APZ_TOPO_CALL` (`name:port`) it echoes each
//! connection back. With it, it relays each connection to that service: the name is asked of the runner's resolver at
//! `APZ_TOPO_RESOLVER`, so a chain of these services reaches each other by name.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use apz_topology::resolver;

fn main() {
    if let Err(why) = run() {
        eprintln!("apz-topo-echo: {why}");
        std::process::exit(1);
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set"))
}

fn run() -> Result<(), String> {
    let bind: Ipv4Addr = env("APZ_TOPO_BIND")?
        .parse()
        .map_err(|e| format!("APZ_TOPO_BIND: {e}"))?;
    let resolver_addr: SocketAddr = env("APZ_TOPO_RESOLVER")?
        .parse()
        .map_err(|e| format!("APZ_TOPO_RESOLVER: {e}"))?;
    let ports: Vec<u16> = env("APZ_TOPO_PORTS")?
        .split(',')
        .map(|port| {
            port.parse::<u16>()
                .map_err(|e| format!("APZ_TOPO_PORTS: {e}"))
        })
        .collect::<Result<_, _>>()?;
    let call: Option<(String, u16)> = match std::env::var("APZ_TOPO_CALL") {
        Ok(text) => {
            let (name, port) = text.split_once(':').ok_or("APZ_TOPO_CALL is name:port")?;
            Some((
                name.to_string(),
                port.parse().map_err(|e| format!("APZ_TOPO_CALL: {e}"))?,
            ))
        }
        Err(_) => None,
    };
    for port in ports {
        let listener =
            TcpListener::bind((bind, port)).map_err(|e| format!("{bind}:{port}: {e}"))?;
        let call = call.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let call = call.clone();
                thread::spawn(move || handle(stream, call, resolver_addr));
            }
        });
    }
    loop {
        thread::park();
    }
}

fn handle(stream: TcpStream, call: Option<(String, u16)>, resolver_addr: SocketAddr) {
    let Some((name, port)) = call else {
        let mut stream = stream;
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        copy(&mut stream, &mut writer);
        return;
    };
    let Ok(addrs) = resolver::lookup(resolver_addr, &name, Duration::from_secs(2)) else {
        return;
    };
    let Some(addr) = addrs.first() else { return };
    let Ok(mut upstream) = TcpStream::connect((*addr, port)) else {
        return;
    };
    let (Ok(mut client_writer), Ok(mut upstream_writer)) =
        (stream.try_clone(), upstream.try_clone())
    else {
        return;
    };
    let mut client = stream;
    let back = thread::spawn(move || copy(&mut upstream, &mut client_writer));
    copy(&mut client, &mut upstream_writer);
    let _ = back.join();
}

/// Copies `from` to `to` until `from` ends, then closes `to`'s write side.
fn copy(from: &mut TcpStream, to: &mut TcpStream) {
    let mut buf = [0u8; 4096];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = to.shutdown(Shutdown::Write);
}
