//! The Compose web stack, converted (PLAN 22.6) and planned, runs through the process runner: its services reach each
//! other by name through the runner's resolver, each starts after its dependencies' ports listen, and its named volume is
//! a directory that lasts the run. The stand-ins are `apz-topo-echo`; the artifacts are not in the repository.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::Duration;

use apz_topology::compose;
use apz_topology::plan::{GuestPlan, plan};
use apz_topology::resolver::lookup;
use apz_topology::run::{Addressing, Standin, start};

const WEB_STACK: &str = include_str!("../tests/fixtures/compose/web-stack.yml");
const TIMEOUT: Duration = Duration::from_secs(5);

/// The web stack's plan: `web` and `api` are artifacts of the release, `db` and `cache` pinned images.
fn web_stack() -> GuestPlan {
    let conversion = compose::convert(WEB_STACK, None).unwrap();
    let artifacts: BTreeMap<String, String> = [("api", "/opt/apz/api"), ("web", "/opt/apz/web")]
        .into_iter()
        .map(|(name, path)| (name.to_string(), path.to_string()))
        .collect();
    plan(&conversion.topology, &artifacts).unwrap()
}

/// Every service is the echo stand-in: web relays to api on its port 8081, api to db on 5432 (the port db exposes).
fn standins() -> BTreeMap<String, Standin> {
    let echo = env!("CARGO_BIN_EXE_apz-topo-echo").to_string();
    let relay = |target: &str| Standin {
        argv: vec![echo.clone()],
        env: [("APZ_TOPO_CALL".to_string(), target.to_string())]
            .into_iter()
            .collect(),
    };
    let plain = || Standin {
        argv: vec![echo.clone()],
        env: BTreeMap::new(),
    };
    [
        ("web", relay("api:8081")),
        ("api", relay("db:5432")),
        ("db", plain()),
        ("cache", plain()),
    ]
    .into_iter()
    .map(|(name, standin)| (name.to_string(), standin))
    .collect()
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "needs the 127.0.1.n loopback addresses of Linux (macOS binds only 127.0.0.1): run with linux.sh"
)]
fn the_converted_web_stack_runs_and_its_services_reach_each_other_by_name() {
    let plan = web_stack();
    let running = start(&plan, Addressing::Loopback, &standins()).unwrap();
    running.wait_listening(Duration::from_secs(10)).unwrap();

    // Each service starts after the services it depends on: api waits for cache and db, web for api.
    let order = running.started_order();
    let at = |name: &str| order.iter().position(|n| n == name).unwrap();
    assert!(
        at("cache-1") < at("api-1") && at("db-1") < at("api-1"),
        "{order:?}"
    );
    assert!(at("api-1") < at("web-1"), "{order:?}");

    // The resolver answers the service names with every replica.
    let api = lookup(running.resolver(), "api", TIMEOUT).unwrap();
    assert_eq!(api.len(), 2, "api has two replicas");
    assert_eq!(api, running.addresses("api"));
    assert_eq!(
        lookup(running.resolver(), "db", TIMEOUT).unwrap(),
        running.addresses("db")
    );

    // A request enters at web (80) and crosses api (8081) to db (5432), and the answer comes back the same way.
    let web: Ipv4Addr = lookup(running.resolver(), "web", TIMEOUT).unwrap()[0];
    let mut connection = TcpStream::connect((web, 80)).unwrap();
    connection.set_read_timeout(Some(TIMEOUT)).unwrap();
    connection.write_all(b"ping").unwrap();
    let mut back = [0u8; 4];
    connection.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"ping");

    // The named volume is a directory for the run, given to the service that mounts it, and removed when the run ends.
    let volume = running
        .volume("db-data")
        .expect("db-data is a volume of the plan")
        .to_path_buf();
    assert!(volume.is_dir());
    drop(running);
    assert!(!volume.exists(), "the run removes its volumes");
}
