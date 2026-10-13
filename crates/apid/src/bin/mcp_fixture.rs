//! The MCP test fixture: a stdio or HTTP MCP server with no dependencies. apid's tests run it.
//! Usage: mcp_fixture stdio | mcp_fixture http PORT
#[path = "../mcp/fixture.rs"]
mod fixture;

use std::io::Write;

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
        _ => {
            eprintln!("usage: mcp_fixture stdio | mcp_fixture http PORT");
            std::process::exit(2);
        }
    }
}
