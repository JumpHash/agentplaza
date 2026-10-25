//! `apz-gui-agent --listen <port>`: serves the lane fork's desktop on 127.0.0.1:<port>, one connection at a time.
//! The X session is the X11 backend's (Xvfb on the PATH, XTest input, XShm capture); the tree is AT-SPI on the session
//! bus named by DBUS_SESSION_BUS_ADDRESS (the fork's command starts that bus).

#[cfg(target_os = "linux")]
fn main() {
    use std::io::BufReader;
    use std::net::TcpListener;
    use std::thread;

    use apz_desktop::{Result as DesktopResult, UiNode};
    use apz_desktop_atspi::AtspiTree;
    use apz_desktop_x11::{UiHook, X11Config, X11Desktop};
    use apz_gui_agent::{Agent, serve, share};
    use apz_station::{Selector, UiAction};

    /// The AT-SPI tree of the fork's own session bus, for whichever display the session draws on.
    struct ForkUi {
        bus: Option<String>,
        tree: Option<AtspiTree>,
    }

    impl ForkUi {
        fn connected(&mut self) -> DesktopResult<&AtspiTree> {
            if self.tree.is_none() {
                let bus = self.bus.as_deref().ok_or_else(|| {
                    apz_desktop::DesktopError::Unsupported(
                        "the tree needs DBUS_SESSION_BUS_ADDRESS (the fork's command starts a session bus)".into(),
                    )
                })?;
                self.tree = Some(AtspiTree::connect(bus)?);
            }
            self.tree.as_ref().ok_or_else(|| {
                apz_desktop::DesktopError::Platform("no accessibility connection".into())
            })
        }
    }

    impl UiHook for ForkUi {
        fn ui_tree(
            &mut self,
            _display: &str,
            app: Option<&str>,
            depth: u8,
        ) -> DesktopResult<UiNode> {
            self.connected()?.tree(app, depth)
        }

        fn ui_act(
            &mut self,
            _display: &str,
            selector: &Selector,
            action: &UiAction,
        ) -> DesktopResult<()> {
            self.connected()?.act(selector, action)
        }
    }

    let port = match listen_port() {
        Ok(port) => port,
        Err(why) => {
            eprintln!("apz-gui-agent: {why}");
            std::process::exit(2);
        }
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(why) => {
            eprintln!("apz-gui-agent: cannot listen on {port}: {why}");
            std::process::exit(1);
        }
    };
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .ok()
        .filter(|v| !v.is_empty());
    let desk = X11Desktop::new(X11Config::default()).with_ui(Box::new(ForkUi { bus, tree: None }));
    let desk = share(Box::new(desk));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Ok(reader) = stream.try_clone() else {
            continue;
        };
        let desk = desk.clone();
        thread::spawn(move || {
            let mut agent = Agent::new(desk);
            if let Err(why) = serve(&mut agent, BufReader::new(reader), stream) {
                eprintln!("apz-gui-agent: a connection ended: {why}");
            }
        });
    }

    fn listen_port() -> Result<u16, String> {
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--listen" {
                let value = args.next().ok_or("--listen names a port")?;
                return value
                    .parse()
                    .map_err(|why| format!("--listen {value}: {why}"));
            }
        }
        Err("usage: apz-gui-agent --listen <port>".into())
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("apz-gui-agent serves a Linux desktop only");
    std::process::exit(1);
}
