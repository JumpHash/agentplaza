//! The MCP test fixture: a stdio or HTTP MCP server with no dependencies. apid's tests run it.
//! Usage: mcp_fixture stdio | mcp_fixture http PORT | mcp_fixture connect ADDR
#[path = "../mcp/fixture.rs"]
mod fixture;

use std::io::Write;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    fixture::mode_exit_at_start();
    match args.get(1).map(String::as_str) {
        Some("stdio") => fixture::serve_stdio(),
        Some("http") => {
            let port: u16 = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
            let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind");
            println!("{}", listener.local_addr().expect("local addr").port()); // tests read the port from stdout
            std::io::stdout().flush().ok();
            fixture::serve_http(listener);
        }
        Some("connect") => {
            // One TCP connection, and what happened: the sandbox tests read whether the sandbox let it through.
            let addr: std::net::SocketAddr = args
                .get(2)
                .and_then(|a| a.parse().ok())
                .expect("an address");
            match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
                Ok(_) => println!("connected"),
                Err(e) => println!("refused: {e}"),
            }
            std::io::stdout().flush().ok();
        }
        _ => {
            eprintln!(
                "usage: mcp_fixture stdio | mcp_fixture http PORT | mcp_fixture connect ADDR"
            );
            std::process::exit(2);
        }
    }
}
