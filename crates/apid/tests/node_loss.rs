//! A multi-VM topology's node loss, driven through the hub's node stop (PLAN #59) on a scratch hub. A `stop_node`
//! scenario stops the node that runs its machine through the hub's own route (`POST /v1/nodes/{id}/stop`), holds it
//! there, puts it back through the resume route, and the probes judge the degradation the spec declares. The
//! dependants' probe server stands in for the services that lose their machine: it answers 503 while the node is out.

mod common;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use apz_topology::node_loss::{self, NodeStop};
use apz_topology::resilience::{Ran, Schedule, judge};
use apz_topology::spec::parse;
use serde_json::json;

use common::{Hub, Scratch};

/// Two machines on the hub's local node (`n0`), `web` on the edge and `db` on the data machine, and a node loss of
/// the data machine: `web` is probed, and the spec allows it to fail while the node is out.
const TOPOLOGY: &str = "name = \"pair\"\n\
[[service]]\nname = \"web\"\nimage = \"web:1\"\n\
[[service]]\nname = \"db\"\nimage = \"postgres:17\"\n\
[[machine]]\nname = \"edge\"\nservices = [\"web\"]\n\
[[machine]]\nname = \"data\"\nservices = [\"db\"]\n\
[[resilience]]\nid = \"lose-data\"\naction = \"stop_node\"\ntarget = \"data\"\nat_secs = 0\nfor_secs = 1\nrecover_within_secs = 2\nprobe = { service = \"web\", path = \"/\" }\nexpect = { max_error_percent = 100.0 }\n";

/// The hub's node stop: each machine's node is one of the hub's nodes, and the stop and the resume are the routes
/// the owner uses. The states the routes answered are recorded in order.
struct RouteNodes<'a> {
    hub: &'a Hub,
    placement: BTreeMap<String, String>,
    down: Arc<AtomicBool>,
    answered: Vec<String>,
}

impl RouteNodes<'_> {
    fn node(&self, machine: &str) -> Result<String, String> {
        self.placement
            .get(machine)
            .cloned()
            .ok_or_else(|| format!("no node runs machine {machine}"))
    }
}

impl NodeStop for RouteNodes<'_> {
    fn stop(&mut self, machine: &str) -> Result<(), String> {
        let node = self.node(machine)?;
        // The dependants are down from the moment the node is lost, not from the route's answer: on a loaded runner
        // the answer took longer than the scenario's hold, and the probe never saw the loss.
        self.down.store(true, Ordering::SeqCst);
        let (status, body) =
            self.hub
                .call("POST", &format!("/v1/nodes/{node}/stop"), Some(json!({})));
        if status != 200 {
            self.down.store(false, Ordering::SeqCst);
            return Err(format!(
                "the route refused the stop of {node}: {status} {body}"
            ));
        }
        self.answered
            .push(body["state"].as_str().unwrap_or_default().to_owned());
        Ok(())
    }

    fn resume(&mut self, machine: &str) -> Result<(), String> {
        let node = self.node(machine)?;
        let (status, body) = self.hub.call(
            "POST",
            &format!("/v1/nodes/{node}/resume"),
            Some(json!({"rebalance": false})),
        );
        if status != 200 {
            return Err(format!(
                "the route refused the resume of {node}: {status} {body}"
            ));
        }
        self.answered
            .push(body["state"].as_str().unwrap_or_default().to_owned());
        self.down.store(false, Ordering::SeqCst);
        Ok(())
    }
}

/// The hub's view of node `id`, from its node list (`GET /v1/nodes`).
fn node_view(hub: &Hub, id: &str) -> serde_json::Value {
    let (status, list) = hub.call("GET", "/v1/nodes", None);
    assert_eq!(status, 200, "the hub lists its nodes: {list}");
    list.as_array()
        .and_then(|nodes| nodes.iter().find(|node| node["id"] == id).cloned())
        .unwrap_or_else(|| panic!("the hub has no node {id}: {list}"))
}

/// A probe server answering 503 while `down` is set and 200 otherwise.
fn serve(down: Arc<AtomicBool>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let status = if down.load(Ordering::SeqCst) {
                "503 Service Unavailable"
            } else {
                "200 OK"
            };
            let _ = stream.write_all(
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
        }
    });
    format!("http://{addr}/")
}

#[test]
fn a_scenario_stops_the_node_through_the_hub_route_and_the_judge_sees_the_declared_degradation() {
    let scratch = Scratch::new("node-loss");
    let hub = Hub::start(&scratch, None, None);
    let topology = parse(TOPOLOGY).unwrap();
    let scenario = topology.resilience[0].clone();

    // Before anything: the hub's local node is in service.
    assert_eq!(node_view(&hub, "n0")["state"], "active");

    let down = Arc::new(AtomicBool::new(false));
    let url = serve(Arc::clone(&down));
    let placement = BTreeMap::from([
        ("edge".to_owned(), "n0".to_owned()),
        ("data".to_owned(), "n0".to_owned()),
    ]);
    let mut nodes = RouteNodes {
        hub: &hub,
        placement,
        down,
        answered: Vec::new(),
    };
    let schedule = Schedule {
        at: Duration::from_millis(0),
        hold: Duration::from_millis(600),
        recover: Duration::from_millis(1200),
        every: Duration::from_millis(20),
    };
    let ran = node_loss::run(&topology, &scenario, &url, schedule, &mut nodes);

    assert_eq!(
        nodes.answered,
        ["stopped", "active"],
        "the route stopped the node, then resumed it"
    );
    let Ran::Measured(measured) = ran else {
        panic!("the node loss did not run");
    };
    assert!(
        measured.failed > 0,
        "the dependant saw the loss while the node was out: {measured:?}"
    );
    assert!(
        judge(&scenario, &measured).passed,
        "the declared degradation holds"
    );

    // The route's own state after the resume: in service again.
    let view = node_view(&hub, "n0");
    assert_eq!(
        view["state"], "active",
        "the node is back in service: {view}"
    );
}
