//! apz-fault: a fault proxy process (PLAN 22.8). The topology's runner starts one per port of a dependency that a
//! resilience scenario faults, with its control on loopback; the scenarios steer it through the control while they run.
//!
//! Usage: `apz-fault proxy <listen> <upstream> <control>`: forwards `listen` to `upstream` under the fault the control
//! sets (`fault <json>` lines, answered `ok`; `stats` answers the proxy's stats). Healthy until steered; runs until it is
//! killed.

use std::net::{SocketAddr, TcpListener};
use std::process::ExitCode;

use apz_topology::fault::{Fault, Proxy, serve_control};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, listen, upstream, control] = args.as_slice() else {
        eprintln!("usage: apz-fault proxy <listen> <upstream> <control>");
        return ExitCode::from(2);
    };
    if mode.as_str() != "proxy" {
        eprintln!("usage: apz-fault proxy <listen> <upstream> <control>");
        return ExitCode::from(2);
    }
    match serve(listen, upstream, control) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("apz-fault: {why}");
            ExitCode::FAILURE
        }
    }
}

fn address(text: &str, what: &str) -> Result<SocketAddr, String> {
    text.parse()
        .map_err(|_| format!("{what} {text:?} is not an address"))
}

fn serve(listen: &str, upstream: &str, control: &str) -> Result<(), String> {
    let proxy = Proxy::start(
        address(listen, "listen")?,
        address(upstream, "upstream")?,
        Fault::Healthy,
    )
    .map_err(|e| format!("{listen}: {e}"))?;
    let listener =
        TcpListener::bind(address(control, "control")?).map_err(|e| format!("{control}: {e}"))?;
    serve_control(listener, proxy.handle()).map_err(|e| format!("control: {e}"))
}
