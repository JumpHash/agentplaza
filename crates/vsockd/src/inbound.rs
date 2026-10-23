//! vsockd's inbound listener (PLAN 6.1): the hub's port streams, handed to it by the node's relay, go to a guest's port
//! once noded says a live run serves that port. The protocol and the relay are `apz_link::inbound`; this file is the
//! daemon's two ends of it: the check, asked of noded over loopback with the node's internal token, and the dial, a vsock
//! connection to the guest (Linux; other platforms refuse every inbound stream).

use std::io;
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use apz_http::auth::InternalToken;
use apz_http::client;
use apz_link::inbound::{Duplex, GuestDial, PortCheck, serve};

/// How long noded may take to answer a check.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Asks noded whether a live run serves a port on a guest (`GET /v1/forks/{cid}/ports/{port}`). Without a noded port,
/// nothing is served.
pub struct NodedCheck {
    pub noded: u16,
    pub token: InternalToken,
}

impl PortCheck for NodedCheck {
    fn served(&self, cid: u64, port: u16) -> bool {
        if self.noded == 0 {
            return false;
        }
        let header = self.token.header();
        let headers: Vec<(&str, &str)> = header
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let path = format!("/v1/forks/{cid}/ports/{port}");
        matches!(
            client::send_with(self.noded, "GET", &path, &headers, None, CHECK_TIMEOUT),
            Ok(reply) if reply.status == 200
        )
    }
}

/// The guest's port, over vsock.
pub struct VsockDial;

impl GuestDial for VsockDial {
    fn dial(&self, cid: u64, port: u16) -> io::Result<Box<dyn Duplex>> {
        let cid = u32::try_from(cid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the CID is out of range"))?;
        vsock_dial(cid, u32::from(port))
    }
}

#[cfg(target_os = "linux")]
fn vsock_dial(cid: u32, port: u32) -> io::Result<Box<dyn Duplex>> {
    let socket = crate::sys::vsock_connect(cid, port)?;
    Ok(Box::new(VsockConn(std::fs::File::from(socket))))
}

/// A vsock connection to a guest, as the relay reads and writes it.
#[cfg(target_os = "linux")]
struct VsockConn(std::fs::File);

#[cfg(target_os = "linux")]
impl io::Read for VsockConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

#[cfg(target_os = "linux")]
impl io::Write for VsockConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(not(target_os = "linux"))]
fn vsock_dial(cid: u32, port: u32) -> io::Result<Box<dyn Duplex>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("no vsock to guest {cid}:{port} on this platform"),
    ))
}

#[cfg(target_os = "linux")]
impl Duplex for VsockConn {
    fn duplicate(&self) -> io::Result<Box<dyn Duplex>> {
        Ok(Box::new(VsockConn(self.0.try_clone()?)))
    }

    fn close_write(&self) -> io::Result<()> {
        crate::sys::shutdown_write(std::os::fd::AsFd::as_fd(&self.0))
    }
}

/// Binds vsockd's inbound listener on loopback and serves it on its own thread.
pub fn start(port: u16, check: NodedCheck) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    println!("vsockd: inbound streams on 127.0.0.1:{port}");
    let check: Arc<dyn PortCheck> = Arc::new(check);
    let dial: Arc<dyn GuestDial> = Arc::new(VsockDial);
    thread::spawn(move || serve(listener, check, dial));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_http::{Response, Router, Server};
    use serde_json::json;

    /// A noded that serves one port, on loopback.
    fn fake_noded() -> apz_http::Handle {
        let router: Router<()> =
            Router::new().get("/v1/forks/{cid}/ports/{port}", |_, _, params| {
                if params.get("cid") == "1000" && params.get("port") == "7681" {
                    Response::json(200, &json!({"served": true}))
                } else {
                    Response::json(404, &json!({"served": false}))
                }
            });
        Server::bind(0)
            .expect("an ephemeral port")
            .spawn(move |request| router.handle(&(), request))
            .expect("serves")
    }

    #[test]
    fn noded_says_which_ports_a_live_run_serves() {
        let noded = fake_noded();
        let check = NodedCheck {
            noded: noded.port(),
            token: InternalToken::none(),
        };
        assert!(check.served(1000, 7681));
        assert!(!check.served(1000, 7682), "an unregistered port");
        let none = NodedCheck {
            noded: 0,
            token: InternalToken::none(),
        };
        assert!(
            !none.served(1000, 7681),
            "without a noded nothing is served"
        );
    }

    /// The dial of an inbound line whose port is not served must never run.
    struct NoDial;

    impl GuestDial for NoDial {
        fn dial(&self, cid: u64, port: u16) -> io::Result<Box<dyn Duplex>> {
            panic!("dialled {cid}:{port}, which no run serves");
        }
    }

    #[test]
    fn an_unregistered_port_is_refused_before_any_dial() {
        let noded = fake_noded();
        let check = NodedCheck {
            noded: noded.port(),
            token: InternalToken::none(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let port = listener.local_addr().expect("address").port();
        let check: Arc<dyn PortCheck> = Arc::new(check);
        let dial: Arc<dyn GuestDial> = Arc::new(NoDial);
        thread::spawn(move || serve(listener, check, dial));
        let refused = apz_link::inbound::connect(port, 1000, 7682).expect_err("refused");
        assert!(refused.to_string().contains("no live run"), "{refused}");
    }

    #[test]
    fn a_dial_that_is_not_a_guest_fails_without_a_panic() {
        assert!(VsockDial.dial(u64::from(u32::MAX) + 1, 7681).is_err());
        assert!(VsockDial.dial(u64::from(u32::MAX), 7681).is_err());
    }
}
