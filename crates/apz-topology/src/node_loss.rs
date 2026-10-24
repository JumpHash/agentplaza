//! A node loss in a multi-VM topology (PLAN 22.6, W58, and the node stop of PLAN #59): a `stop_node` scenario takes the
//! node that runs one of the topology's machines out of service, holds it there, puts it back, and the probes judge the
//! degradation the spec declares, as for every resilience scenario ([`crate::resilience::judge`]).
//!
//! The step is not run in the guest: a node is the hub's to stop. The hub supplies a [`NodeStop`] over its node stop
//! (`POST /v1/nodes/{id}/stop`, then the node's resume), and [`run`] drives the scenario through it with the same
//! schedule and probes the guest-side scenarios use ([`crate::resilience::run`]).

use crate::resilience::{Actuator, Ran, Schedule, run as run_scenario};
use crate::spec::{Action, Scenario, Topology};

/// Takes the node a machine runs on out of service, and puts it back. The hub implements this over its node stop.
pub trait NodeStop {
    /// Stops the node that runs `machine`: no new work lands on it, and its agents move off it.
    fn stop(&mut self, machine: &str) -> Result<(), String>;
    /// Puts the node that runs `machine` back in service.
    fn resume(&mut self, machine: &str) -> Result<(), String>;
}

/// Applies a topology's `stop_node` scenarios through a [`NodeStop`]. The other actions run in the guest and are refused
/// here.
pub struct NodeActuator<'a, N: NodeStop> {
    topology: &'a Topology,
    nodes: &'a mut N,
}

impl<'a, N: NodeStop> NodeActuator<'a, N> {
    /// An actuator for `topology`'s node losses, through `nodes`.
    pub fn new(topology: &'a Topology, nodes: &'a mut N) -> Self {
        Self { topology, nodes }
    }
}

impl<N: NodeStop> Actuator for NodeActuator<'_, N> {
    fn available(&self, scenario: &Scenario) -> Result<(), String> {
        if scenario.action != Action::StopNode {
            return Err(format!(
                "scenario {} is not a node stop: the guest runs it",
                scenario.id
            ));
        }
        if !self
            .topology
            .machines
            .iter()
            .any(|machine| machine.name == scenario.target)
        {
            return Err(format!(
                "scenario {} stops {}, which is no machine of {}",
                scenario.id, scenario.target, self.topology.name
            ));
        }
        Ok(())
    }

    fn act(&mut self, scenario: &Scenario, on: bool) -> Result<(), String> {
        if scenario.action != Action::StopNode {
            return Err(format!("scenario {} is not a node stop", scenario.id));
        }
        if on {
            self.nodes.stop(&scenario.target)
        } else {
            self.nodes.resume(&scenario.target)
        }
    }
}

/// Runs one `stop_node` scenario of `topology` against `url` (a probe of a service that depends on the lost machine),
/// stopping the machine's node at the schedule's start and resuming it when the hold ends. The result is the probes'
/// measurement, or why the scenario did not run; judge it with [`crate::resilience::judge`].
pub fn run<N: NodeStop>(
    topology: &Topology,
    scenario: &Scenario,
    url: &str,
    schedule: Schedule,
    nodes: &mut N,
) -> Ran {
    let mut actuator = NodeActuator::new(topology, nodes);
    run_scenario(scenario, url, schedule, &mut actuator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilience::judge;
    use crate::spec::parse;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    /// A two-VM topology: `web` on the edge machine, `db` on the data machine, and a node loss of the data machine.
    fn pair(scenario: &str) -> String {
        format!(
            "name = \"pair\"\n\
             [[service]]\nname = \"web\"\nimage = \"web:1\"\n\
             [[service]]\nname = \"db\"\nimage = \"postgres:17\"\n\
             [[machine]]\nname = \"edge\"\nservices = [\"web\"]\n\
             [[machine]]\nname = \"data\"\nservices = [\"db\"]\n\
             [[resilience]]\n{scenario}"
        )
    }

    const LOSE_DATA: &str = "id = \"lose-data\"\naction = \"stop_node\"\ntarget = \"data\"\nat_secs = 0\nfor_secs = 1\nrecover_within_secs = 2\nprobe = { service = \"web\", path = \"/\" }\nexpect = { max_error_percent = 100.0 }\n";

    /// A node stop that records its calls and tells the probe server the lost machine's dependants are down.
    #[derive(Default)]
    struct Nodes {
        down: Arc<AtomicBool>,
        calls: Vec<String>,
    }

    impl NodeStop for Nodes {
        fn stop(&mut self, machine: &str) -> Result<(), String> {
            self.calls.push(format!("stop {machine}"));
            self.down.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn resume(&mut self, machine: &str) -> Result<(), String> {
            self.calls.push(format!("resume {machine}"));
            self.down.store(false, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A local HTTP server that answers 503 while `down` is set, and 200 otherwise.
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

    fn schedule() -> Schedule {
        Schedule {
            at: Duration::from_millis(0),
            hold: Duration::from_millis(500),
            recover: Duration::from_millis(800),
            every: Duration::from_millis(20),
        }
    }

    #[test]
    fn a_node_loss_is_a_scenario_of_a_multi_vm_topology_and_names_a_machine() {
        let topology = parse(&pair(LOSE_DATA)).unwrap();
        assert_eq!(topology.machines.len(), 2);
        assert_eq!(topology.resilience[0].action, Action::StopNode);

        // One machine is not a multi-VM topology: a node stop has nothing to take out of service.
        let single = "name = \"solo\"\n[[service]]\nname = \"web\"\nimage = \"web:1\"\n[[machine]]\nname = \"edge\"\nservices = [\"web\"]\n[[resilience]]\n".to_owned() + LOSE_DATA;
        assert!(parse(&single).unwrap_err().contains("multi-VM"));

        // A machine that is not declared, and a service on no machine or on two, are refused.
        let undeclared = pair(LOSE_DATA).replace("target = \"data\"", "target = \"cache\"");
        assert!(parse(&undeclared).unwrap_err().contains("no machine"));
        let unplaced = pair(LOSE_DATA).replace("services = [\"db\"]", "services = []");
        assert!(parse(&unplaced).unwrap_err().contains("runs no services"));
        let twice = pair(LOSE_DATA).replace("services = [\"db\"]", "services = [\"web\", \"db\"]");
        assert!(parse(&twice).unwrap_err().contains("two machines"));
        let stray =
            pair(LOSE_DATA).replace("services = [\"db\"]", "services = [\"db\", \"cache\"]");
        assert!(parse(&stray).unwrap_err().contains("no service"));

        // A stop takes no fault, volume or clock skew.
        let with_fault = pair(LOSE_DATA).replace(
            "target = \"data\"",
            "target = \"data\"\nfault = { kind = \"reset\", percent = 30 }",
        );
        assert!(parse(&with_fault).unwrap_err().contains("fault"));
    }

    #[test]
    fn the_actuator_stops_the_machines_node_and_refuses_what_the_guest_runs() {
        let topology = parse(&pair(LOSE_DATA)).unwrap();
        let scenario = &topology.resilience[0];
        let mut nodes = Nodes::default();
        let mut actuator = NodeActuator::new(&topology, &mut nodes);
        assert!(actuator.available(scenario).is_ok());
        actuator.act(scenario, true).unwrap();
        actuator.act(scenario, false).unwrap();
        assert_eq!(nodes.calls, ["stop data", "resume data"]);

        // A kill is run in the guest, not here.
        let kill = parse(&pair(&LOSE_DATA.replace(
            "action = \"stop_node\"\ntarget = \"data\"",
            "action = \"kill\"\ntarget = \"db\"",
        )))
        .unwrap();
        let mut nodes = Nodes::default();
        let actuator = NodeActuator::new(&kill, &mut nodes);
        assert!(actuator.available(&kill.resilience[0]).is_err());
    }

    #[test]
    fn a_node_loss_run_probes_the_dependants_and_judges_the_declared_degradation() {
        let topology = parse(&pair(LOSE_DATA)).unwrap();
        let scenario = topology.resilience[0].clone();
        let mut nodes = Nodes::default();
        let url = serve(Arc::clone(&nodes.down));

        let ran = run(&topology, &scenario, &url, schedule(), &mut nodes);
        assert_eq!(
            nodes.calls,
            ["stop data", "resume data"],
            "the node is stopped and put back"
        );
        let Ran::Measured(measured) = ran else {
            panic!("the node loss did not run");
        };
        assert!(
            measured.failed > 0,
            "the probes saw the loss while the node was out: {measured:?}"
        );
        assert!(
            judge(&scenario, &measured).passed,
            "the declared 100 % degradation holds"
        );

        // A spec that allows no errors fails the same run: the degradation is what the spec declares, no more.
        let mut strict = scenario.clone();
        strict.expect.max_error_percent = 0.0;
        assert!(!judge(&strict, &measured).passed);
    }
}
