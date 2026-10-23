//! The guest config of a k3s topology (PLAN 22.6): a Kubernetes cluster (k3s) the lab VM runs for a project that deploys to
//! Kubernetes. Pure: [`guest_files`] names every file the guest needs and what it says. The lab base writes them, and the
//! guest's init starts [`PROGRAM`] as a process, as it starts the runner of a services topology.
//!
//! The cluster is one server node. k3s applies the topology's manifests itself from its auto-deploy directory when the
//! server starts. The images the topology names are preloaded into the cluster's containerd from image archives the lab
//! base holds ([`IMAGES`] lists them), and the containerd's remaining pulls go through the guest's egress proxy at
//! `127.0.0.1:3128`, as a services topology's containers do. The kernel features the cluster needs are in
//! `scripts/guest/k3s.config`, merged into the lab kernel.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::spec::{Kind, Topology};

/// The server's config file.
pub const CONFIG: &str = "/etc/rancher/k3s/config.yaml";
/// The guest's program that starts the server (a shell script, run as a process by the guest's init).
pub const PROGRAM: &str = "/usr/local/sbin/apz-k3s";
/// The sysctls a cluster needs (applied by [`PROGRAM`]).
pub const SYSCTL: &str = "/etc/sysctl.d/90-apz-k3s.conf";
/// The kernel modules a cluster needs (loaded by [`PROGRAM`]).
pub const MODULES: &str = "/etc/modules-load.d/apz-k3s.conf";
/// The images the lab base preloads into the cluster's containerd, one per line.
pub const IMAGES: &str = "/etc/apz/k3s-images.txt";
/// The directory k3s applies manifests from when the server starts.
pub const AUTO_DEPLOY: &str = "/var/lib/rancher/k3s/server/manifests";
/// The k3s binary the lab base holds (its pinned release).
pub const BINARY: &str = "/usr/local/bin/k3s";
/// The guest's egress proxy, reached over vsock.
pub const PROXY: &str = "http://127.0.0.1:3128";
/// What the server never sends through the proxy: the loopback, the cluster's pod and service ranges, and its names.
pub const NO_PROXY: &str = "127.0.0.1,localhost,10.42.0.0/16,10.43.0.0/16,.svc,.cluster.local";

/// One file of the guest: its path in the guest, its mode, and its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestFile {
    pub path: String,
    pub mode: u32,
    pub contents: String,
}

/// The server's config, as k3s reads it.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Config<'a> {
    node_name: &'a str,
    write_kubeconfig_mode: &'static str,
    disable: &'static [&'static str],
}

/// The guest files of a k3s topology. `manifests` gives the text of each manifest the topology names (by its repo path).
/// The topology is checked first; a manifest that is missing or is not YAML is refused by its path.
pub fn guest_files(
    topology: &Topology,
    manifests: &BTreeMap<String, String>,
) -> Result<Vec<GuestFile>, String> {
    topology.check()?;
    if topology.kind != Kind::K3s {
        return Err(format!("topology {} is not a k3s cluster", topology.name));
    }
    let k3s = topology
        .k3s
        .as_ref()
        .ok_or_else(|| format!("topology {} has no [k3s]", topology.name))?;
    let mut files = vec![
        GuestFile {
            path: CONFIG.into(),
            mode: 0o644,
            contents: config(&topology.name)?,
        },
        GuestFile {
            path: PROGRAM.into(),
            mode: 0o755,
            contents: program(),
        },
        GuestFile {
            path: SYSCTL.into(),
            mode: 0o644,
            contents: "net.ipv4.ip_forward = 1\nnet.bridge.bridge-nf-call-iptables = 1\nnet.bridge.bridge-nf-call-ip6tables = 1\n".into(),
        },
        GuestFile {
            path: MODULES.into(),
            mode: 0o644,
            contents: "overlay\nbr_netfilter\n".into(),
        },
        GuestFile {
            path: IMAGES.into(),
            mode: 0o644,
            contents: images(&k3s.images),
        },
    ];
    for (index, path) in k3s.manifests.iter().enumerate() {
        let text = manifests
            .get(path)
            .ok_or_else(|| format!("k3s manifest {path} is not given"))?;
        check_yaml(path, text)?;
        files.push(GuestFile {
            path: format!("{AUTO_DEPLOY}/apz-{}.yaml", index + 1),
            mode: 0o644,
            contents: text.clone(),
        });
    }
    Ok(files)
}

fn config(node: &str) -> Result<String, String> {
    serde_yaml_ng::to_string(&Config {
        node_name: node,
        write_kubeconfig_mode: "0644",
        disable: &["traefik"],
    })
    .map_err(|e| format!("k3s config: {e}"))
}

/// The guest's program: the modules and sysctls, the proxy for the pulls, then the server with its config. Shell, so the
/// guest's busybox runs it.
fn program() -> String {
    [
        "#!/bin/sh".to_string(),
        "# The lab VM's k3s server (PLAN 22.6), started by the guest's init as a process: the modules and sysctls a cluster".into(),
        "# needs, the guest's egress proxy for the pulls that remain, then the server with its config.".into(),
        "set -eu".into(),
        "modprobe overlay".into(),
        "modprobe br_netfilter".into(),
        format!("sysctl -p {SYSCTL}"),
        format!("export HTTP_PROXY={PROXY}"),
        format!("export HTTPS_PROXY={PROXY}"),
        format!("export NO_PROXY={NO_PROXY}"),
        format!("exec {BINARY} server --config {CONFIG}"),
        String::new(),
    ]
    .join("\n")
}

fn images(images: &[String]) -> String {
    let mut text = String::from(
        "# The images the lab base preloads into the cluster's containerd (k3s imports the archives in its agent images directory).\n",
    );
    for image in images {
        text.push_str(image);
        text.push('\n');
    }
    text
}

/// Every YAML document of `text` parses; a manifest with no document is refused.
fn check_yaml(path: &str, text: &str) -> Result<(), String> {
    let mut documents = 0;
    for document in serde_yaml_ng::Deserializer::from_str(text) {
        serde::Deserialize::deserialize(document)
            .map(|_: Value| documents += 1)
            .map_err(|e| format!("k3s manifest {path} is not YAML: {e}"))?;
    }
    if documents == 0 {
        return Err(format!("k3s manifest {path} has no document"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::*;
    use crate::spec;

    const CLUSTER: &str = "name = \"cluster\"\nkind = \"k3s\"\n[k3s]\nversion = \"v1.31.4+k3s1\"\nmanifests = [\".plaza/topologies/cluster/app.yaml\"]\nimages = [\"postgres:17\", \"redis:7\"]\n";
    const APP: &str = "kind: Deployment\nmetadata: {name: api}\nspec:\n  replicas: 2\n";

    fn manifests() -> BTreeMap<String, String> {
        [(
            ".plaza/topologies/cluster/app.yaml".to_string(),
            APP.to_string(),
        )]
        .into_iter()
        .collect()
    }

    fn file<'a>(files: &'a [GuestFile], path: &str) -> &'a GuestFile {
        files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no {path}"))
    }

    #[test]
    fn the_guest_gets_its_config_program_modules_sysctls_images_and_manifests() {
        let topology = spec::parse(CLUSTER).unwrap();
        let files = guest_files(&topology, &manifests()).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                CONFIG,
                PROGRAM,
                SYSCTL,
                MODULES,
                IMAGES,
                "/var/lib/rancher/k3s/server/manifests/apz-1.yaml"
            ]
        );
        assert_eq!(file(&files, PROGRAM).mode, 0o755);
        assert_eq!(file(&files, CONFIG).mode, 0o644);
        assert!(
            file(&files, IMAGES)
                .contents
                .contains("postgres:17\nredis:7\n")
        );
        assert_eq!(
            file(&files, "/var/lib/rancher/k3s/server/manifests/apz-1.yaml").contents,
            APP
        );
    }

    #[test]
    fn the_server_config_is_the_clusters_name_with_traefik_off() {
        let topology = spec::parse(CLUSTER).unwrap();
        let files = guest_files(&topology, &manifests()).unwrap();
        let config: Value = serde_yaml_ng::from_str(&file(&files, CONFIG).contents).unwrap();
        assert_eq!(config["node-name"], "cluster");
        assert_eq!(config["write-kubeconfig-mode"], "0644");
        assert_eq!(config["disable"], serde_json::json!(["traefik"]));
    }

    #[test]
    fn the_program_runs_the_server_behind_the_guests_proxy_after_its_modules_and_sysctls() {
        let topology = spec::parse(CLUSTER).unwrap();
        let files = guest_files(&topology, &manifests()).unwrap();
        let program = &file(&files, PROGRAM).contents;
        assert!(program.starts_with("#!/bin/sh\n"));
        assert!(program.contains("export HTTPS_PROXY=http://127.0.0.1:3128\n"));
        assert!(program.contains("export NO_PROXY=127.0.0.1,localhost,10.42.0.0/16"));
        let load = program.find("modprobe br_netfilter").unwrap();
        let proxy = program.find("export HTTP_PROXY").unwrap();
        let start = program
            .find("exec /usr/local/bin/k3s server --config")
            .unwrap();
        assert!(load < proxy && proxy < start, "{program}");
        // The shell reads it: a syntax error would fail the guest's start.
        let mut sh = Command::new("sh")
            .arg("-n")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        sh.stdin
            .take()
            .unwrap()
            .write_all(program.as_bytes())
            .unwrap();
        assert!(sh.wait().unwrap().success());
    }

    #[test]
    fn the_modules_and_sysctls_are_the_ones_a_cluster_needs() {
        let topology = spec::parse(CLUSTER).unwrap();
        let files = guest_files(&topology, &manifests()).unwrap();
        assert!(
            file(&files, MODULES)
                .contents
                .lines()
                .any(|l| l == "overlay")
        );
        assert!(
            file(&files, MODULES)
                .contents
                .lines()
                .any(|l| l == "br_netfilter")
        );
        assert!(
            file(&files, SYSCTL)
                .contents
                .contains("net.ipv4.ip_forward = 1")
        );
    }

    #[test]
    fn the_kernel_fragment_names_what_the_cluster_needs() {
        let fragment = format!(
            "{}{}",
            include_str!("../../../scripts/guest/lab.config"),
            include_str!("../../../scripts/guest/k3s.config")
        );
        for symbol in [
            "CONFIG_CGROUPS",
            "CONFIG_CGROUP_PIDS",
            "CONFIG_MEMCG",
            "CONFIG_CPUSETS",
            "CONFIG_NAMESPACES",
            "CONFIG_NET_NS",
            "CONFIG_OVERLAY_FS",
            "CONFIG_BRIDGE",
            "CONFIG_BRIDGE_NETFILTER",
            "CONFIG_VETH",
            "CONFIG_VXLAN",
            "CONFIG_NETFILTER",
            "CONFIG_NF_CONNTRACK",
            "CONFIG_NF_NAT",
            "CONFIG_IP_NF_IPTABLES",
            "CONFIG_IP_NF_NAT",
            "CONFIG_NETFILTER_XT_MATCH_CONNTRACK",
            "CONFIG_NETFILTER_XT_TARGET_MASQUERADE",
            "CONFIG_PROC_FS",
            "CONFIG_SYSFS",
        ] {
            assert!(
                fragment.lines().any(|line| line == format!("{symbol}=y")),
                "{symbol} is not in k3s.config"
            );
        }
    }

    #[test]
    fn a_services_topology_has_no_cluster_config_and_a_bad_manifest_is_refused() {
        let services =
            spec::parse("name = \"web\"\n[[service]]\nname = \"api\"\nimage = \"y:1\"\n").unwrap();
        assert!(
            guest_files(&services, &BTreeMap::new())
                .unwrap_err()
                .contains("not a k3s cluster")
        );
        let topology = spec::parse(CLUSTER).unwrap();
        assert!(
            guest_files(&topology, &BTreeMap::new())
                .unwrap_err()
                .contains("app.yaml")
        );
        let broken: BTreeMap<String, String> = [(
            ".plaza/topologies/cluster/app.yaml".to_string(),
            "kind: [unclosed".to_string(),
        )]
        .into_iter()
        .collect();
        assert!(
            guest_files(&topology, &broken)
                .unwrap_err()
                .contains("not YAML")
        );
    }
}
