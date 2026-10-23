//! The web-stack example, planned and started as processes: its services reach each other by name through the runner's
//! resolver (PLAN 22.6). The stand-ins are `apz-topo-echo`; the containers' artifacts are not in the repository.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::Duration;

use apz_topology::plan::{GuestPlan, plan};
use apz_topology::resolver::lookup;
use apz_topology::run::{Addressing, Standin, start};
use apz_topology::spec;

const WEB_STACK: &str = include_str!("fixtures/web-stack.toml");
const TIMEOUT: Duration = Duration::from_secs(5);

fn web_stack() -> GuestPlan {
    let artifacts: BTreeMap<String, String> = [
        ("api-linux-x86_64", "/opt/apz/api"),
        ("web-dist", "/opt/apz/web"),
    ]
    .into_iter()
    .map(|(name, path)| (name.to_string(), path.to_string()))
    .collect();
    plan(&spec::parse(WEB_STACK).unwrap(), &artifacts).unwrap()
}

/// Every service is the echo stand-in: web relays to api:8080, api to db:5432 (the port db declares), and db echoes.
fn standins() -> BTreeMap<String, Standin> {
    let echo = env!("CARGO_BIN_EXE_apz-topo-echo").to_string();
    let call = |target: &str| {
        let mut env = BTreeMap::new();
        env.insert("APZ_TOPO_CALL".to_string(), target.to_string());
        Standin {
            argv: vec![echo.clone()],
            env,
        }
    };
    let echo_only = Standin {
        argv: vec![echo.clone()],
        env: BTreeMap::new(),
    };
    let mut map = BTreeMap::new();
    map.insert("web".to_string(), call("api:8080"));
    map.insert("api".to_string(), call("db:5432"));
    map.insert("db".to_string(), echo_only.clone());
    map.insert("cache".to_string(), echo_only);
    map
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "needs the 127.0.1.n loopback addresses of Linux (macOS binds only 127.0.0.1): run with linux.sh"
)]
fn the_web_stack_runs_and_its_services_reach_each_other_by_name() {
    let plan = web_stack();
    let running = start(&plan, Addressing::Loopback, &standins()).unwrap();
    running.wait_listening(Duration::from_secs(10)).unwrap();

    // The runner's resolver answers the service names with every replica, and the containers' own names.
    let api = lookup(running.resolver(), "api", TIMEOUT).unwrap();
    assert_eq!(api.len(), 2);
    assert_eq!(api, running.addresses("api"));
    assert_eq!(
        lookup(running.resolver(), "db", TIMEOUT).unwrap(),
        running.addresses("db")
    );
    assert_eq!(
        lookup(running.resolver(), "api-2", TIMEOUT).unwrap(),
        [api[1]]
    );
    assert!(lookup(running.resolver(), "payments", TIMEOUT).is_err());

    // A request enters at web and crosses api to db, and the answer comes back the same way.
    let web: Ipv4Addr = lookup(running.resolver(), "web", TIMEOUT).unwrap()[0];
    let mut connection = TcpStream::connect((web, 8080)).unwrap();
    connection.set_read_timeout(Some(TIMEOUT)).unwrap();
    connection.write_all(b"ping").unwrap();
    let mut back = [0u8; 4];
    connection.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"ping");
}

#[test]
fn an_image_container_without_a_stand_in_is_refused() {
    let plan = web_stack();
    let mut only_web = standins();
    only_web.remove("db");
    let refused = start(&plan, Addressing::Loopback, &only_web).err().unwrap();
    assert!(
        refused.contains("db-1") && refused.contains("postgres:17"),
        "{refused}"
    );
}
