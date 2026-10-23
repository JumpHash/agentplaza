//! The Compose import (PLAN 22.6): a project's `docker-compose.yml` (or `compose.yaml`) as a services topology.
//!
//! A service becomes a [`Service`]. `image` is its pinned image; `build` makes it an artifact of the release under test,
//! named after the service (the build itself is not run). `environment`, the container ports (`ports` and `expose`:
//! what the service listens on), `depends_on` and `links` (a wait for the dependency's ports), named `volumes`,
//! `healthcheck` and `deploy.replicas` carry over. Every other key is an item of the report: approximated where the lab
//! means it differently, not converted where a topology has no place for it. Interpolation takes a variable's default
//! (`${VAR:-x}`), since the lab has none of the owner's variables; a variable with no default is an item, and its value
//! is left out.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::report::{Conversion, Item, Outcome, note};
use crate::spec::{Health, Kind, MAX_REPLICAS, Service, Topology, Volume};

/// The Compose file in `text`, converted into a topology named `name` (or the file's top-level `name`).
pub fn convert(text: &str, name: Option<&str>) -> Result<Conversion, String> {
    let doc: Value = serde_yaml_ng::from_str(text).map_err(|e| format!("compose file: {e}"))?;
    let top = doc
        .as_object()
        .ok_or("compose file: the top level is not a mapping")?;
    let name = match (name, top.get("name").and_then(Value::as_str)) {
        (Some(name), _) => name.to_string(),
        (None, Some(name)) => name.to_string(),
        (None, None) => {
            return Err(
                "compose file: it has no top-level name, so the topology needs one".to_string(),
            );
        }
    };
    let mut report = Vec::new();
    for (key, value) in top {
        match key.as_str() {
            "name" | "services" => {}
            "version" => note(
                &mut report,
                "version",
                Outcome::Converted,
                "obsolete in Compose too: nothing to carry",
            ),
            "volumes" => top_volumes(value, &mut report),
            extension if extension.starts_with("x-") => note(
                &mut report,
                extension,
                Outcome::Converted,
                "an extension field: Compose ignores it too",
            ),
            "networks" if is_empty(value) => {}
            "networks" => note(
                &mut report,
                "networks",
                Outcome::Approximated,
                "every service shares the lab VM's bridge: the network names are not kept",
            ),
            _ => note(
                &mut report,
                key,
                Outcome::NotConverted,
                "a top-level key a topology does not carry",
            ),
        }
    }
    let services_map = top
        .get("services")
        .and_then(Value::as_object)
        .ok_or("compose file: no services mapping")?;
    let mut services = Vec::new();
    for (service_name, raw) in services_map {
        let raw = raw
            .as_object()
            .ok_or_else(|| format!("compose file: service {service_name} is not a mapping"))?;
        services.push(service(service_name, raw, &mut report)?);
    }
    let topology = Topology {
        name,
        kind: Kind::Services,
        services,
        k3s: None,
        observe: Default::default(),
        traffic: None,
        assertions: Vec::new(),
        egress: Default::default(),
    };
    topology.check()?;
    Ok(Conversion { topology, report })
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        Value::Array(list) => list.is_empty(),
        _ => false,
    }
}

/// The top-level `volumes`: each is a named volume the services may mount; its options are not carried.
fn top_volumes(value: &Value, report: &mut Vec<Item>) {
    let Some(map) = value.as_object() else {
        return;
    };
    for (name, options) in map {
        let Some(options) = options.as_object() else {
            continue;
        };
        if options.get("external").and_then(Value::as_bool) == Some(true) {
            note(
                report,
                format!("volumes.{name}"),
                Outcome::NotConverted,
                "an external volume is the host's: the lab creates its own",
            );
        }
        for key in options.keys().filter(|key| key.as_str() != "external") {
            note(
                report,
                format!("volumes.{name}.{key}"),
                Outcome::NotConverted,
                "a volume option the lab does not keep (its volumes are directories)",
            );
        }
    }
}

/// The keys of a service that have their own conversion in [`service`].
const HANDLED: &[&str] = &[
    "image",
    "build",
    "environment",
    "ports",
    "expose",
    "volumes",
    "depends_on",
    "links",
    "healthcheck",
    "deploy",
    "networks",
];

/// One service of the file, converted.
fn service(
    name: &str,
    raw: &Map<String, Value>,
    report: &mut Vec<Item>,
) -> Result<Service, String> {
    let mut service = Service {
        name: name.to_string(),
        artifact: None,
        image: None,
        replicas: 1,
        ports: Vec::new(),
        env: BTreeMap::new(),
        data: None,
        serve: None,
        depends_on: Vec::new(),
        volumes: Vec::new(),
        health: None,
    };
    match (raw.get("build"), raw.get("image")) {
        (Some(build), image) => {
            service.artifact = Some(name.to_string());
            note(
                report,
                format!("{name}.build"),
                Outcome::Approximated,
                format!(
                    "built by the release under test: the release must produce the artifact {name}; the build is not run"
                ),
            );
            for key in build.as_object().into_iter().flat_map(Map::keys) {
                if !matches!(key.as_str(), "context" | "dockerfile") {
                    note(
                        report,
                        format!("{name}.build.{key}"),
                        Outcome::NotConverted,
                        "the build is not run in the lab",
                    );
                }
            }
            if let Some(image) = image {
                note(
                    report,
                    format!("{name}.image"),
                    Outcome::NotConverted,
                    format!("{} is not run: the build's artifact is", text_of(image)),
                );
            }
        }
        (None, Some(image)) => {
            let image = image
                .as_str()
                .ok_or_else(|| format!("service {name}: image is not a string"))?;
            let image = interpolate(image, &format!("{name}.image"), report);
            if !is_pinned(&image) {
                note(
                    report,
                    format!("{name}.image"),
                    Outcome::Approximated,
                    format!("{image} is not pinned to a tag (latest is not a pin)"),
                );
            }
            service.image = Some(image);
        }
        (None, None) => return Err(format!("service {name} has neither image nor build")),
    }
    environment(name, raw.get("environment"), &mut service, report)?;
    ports(name, raw, &mut service, report)?;
    volumes(name, raw.get("volumes"), &mut service, report)?;
    waits(name, raw, &mut service, report)?;
    healthcheck(name, raw.get("healthcheck"), &mut service, report)?;
    deploy(name, raw.get("deploy"), &mut service, report)?;
    for key in raw.keys() {
        if key.starts_with("x-") {
            note(
                report,
                format!("{name}.{key}"),
                Outcome::Converted,
                "an extension field: Compose ignores it too",
            );
        } else if key == "networks" {
            note(
                report,
                format!("{name}.networks"),
                Outcome::Approximated,
                "every service shares the lab VM's bridge: the network names are not kept",
            );
        } else if !HANDLED.contains(&key.as_str()) {
            let (outcome, why) = unconverted(key);
            note(report, format!("{name}.{key}"), outcome, why);
        }
    }
    Ok(service)
}

/// What a key the import does not convert is: approximated or not converted, and why.
fn unconverted(key: &str) -> (Outcome, &'static str) {
    match key {
        "command" | "entrypoint" => (
            Outcome::NotConverted,
            "the lab starts the artifact or the image's own program: a command is not kept",
        ),
        "env_file" => (
            Outcome::NotConverted,
            "the file is not read: list its variables under environment",
        ),
        "profiles" => (
            Outcome::Approximated,
            "every service of the file is in the topology: profiles are not kept",
        ),
        "restart" => (
            Outcome::NotConverted,
            "the lab does not restart services: kills and restarts belong to the resilience stage (22.8)",
        ),
        "container_name" | "hostname" | "domainname" | "labels" | "logging"
        | "stop_grace_period" | "stop_signal" | "init" | "mem_limit" | "memswap_limit"
        | "mem_reservation" | "cpus" | "cpu_shares" | "cpu_count" | "cpu_quota" | "cpu_period"
        | "pids_limit" | "shm_size" | "ulimits" | "sysctls" | "pid" | "ipc" | "uts" | "user"
        | "working_dir" | "platform" | "security_opt" | "privileged" | "cap_add" | "cap_drop"
        | "devices" | "dns" | "dns_search" | "dns_opt" | "extra_hosts" | "external_links"
        | "read_only" | "stdin_open" | "tty" | "tmpfs" | "secrets" | "configs" | "extends"
        | "develop" | "annotations" | "pull_policy" | "network_mode" | "volumes_from"
        | "group_add" | "runtime" | "isolation" | "cgroup_parent" | "storage_opt"
        | "oom_kill_disable" | "userns_mode" | "gpus" | "scale" | "credential_spec"
        | "mac_address" => (
            Outcome::NotConverted,
            "a host or runtime setting: the lab VM sets it, and a topology does not carry it",
        ),
        _ => (
            Outcome::NotConverted,
            "a key this import does not know: not converted",
        ),
    }
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A scalar as the text of an environment value (numbers and booleans as written).
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

/// An image pinned to a tag or a digest.
fn is_pinned(image: &str) -> bool {
    if image.contains('@') {
        return true;
    }
    let last = image.rsplit('/').next().unwrap_or(image);
    matches!(last.split_once(':'), Some((_, tag)) if !tag.is_empty() && tag != "latest")
}

fn environment(
    name: &str,
    value: Option<&Value>,
    service: &mut Service,
    report: &mut Vec<Item>,
) -> Result<(), String> {
    match value {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (key, value) in map {
                let at = format!("{name}.environment.{key}");
                match scalar(value) {
                    Some(text) => {
                        let text = interpolate(&text, &at, report);
                        service.env.insert(key.clone(), text);
                    }
                    None => shell_value(&at, report),
                }
            }
        }
        Some(Value::Array(list)) => {
            for item in list {
                let text = item
                    .as_str()
                    .ok_or_else(|| format!("service {name}: an environment entry is not text"))?;
                match text.split_once('=') {
                    Some((key, value)) => {
                        let at = format!("{name}.environment.{key}");
                        let value = interpolate(value, &at, report);
                        service.env.insert(key.to_string(), value);
                    }
                    None => shell_value(&format!("{name}.environment.{text}"), report),
                }
            }
        }
        Some(_) => {
            return Err(format!(
                "service {name}: environment is a mapping or a list"
            ));
        }
    }
    Ok(())
}

fn shell_value(at: &str, report: &mut Vec<Item>) {
    note(
        report,
        at,
        Outcome::NotConverted,
        "takes its value from the shell that runs Compose: write the value in the topology",
    );
}

/// The container ports of `ports` and `expose` (TCP, what the service listens on); published host ports and UDP are items.
fn ports(
    name: &str,
    raw: &Map<String, Value>,
    service: &mut Service,
    report: &mut Vec<Item>,
) -> Result<(), String> {
    let mut listen = BTreeSet::new();
    for key in ["ports", "expose"] {
        let Some(list) = raw.get(key).and_then(Value::as_array) else {
            continue;
        };
        for entry in list {
            let at = format!("{name}.{key}");
            let (targets, published, udp) = match entry {
                Value::Object(long) => {
                    let target = long
                        .get("target")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| format!("service {name}: a long port has no target"))?;
                    let target = u16::try_from(target)
                        .map_err(|_| format!("service {name}: port {target} is not a port"))?;
                    let published = long.get("published").is_some();
                    let udp = long.get("protocol").and_then(Value::as_str) == Some("udp");
                    (vec![target], published, udp)
                }
                other => {
                    let spec = short_port(&text_of(other))
                        .map_err(|why| format!("service {name}: {why}"))?;
                    (spec.targets, spec.published, spec.udp)
                }
            };
            if udp {
                note(
                    report,
                    at,
                    Outcome::NotConverted,
                    format!("{targets:?} over UDP: a topology's services listen on TCP"),
                );
                continue;
            }
            if published {
                note(
                    report,
                    at.clone(),
                    Outcome::Approximated,
                    "published on the host: not kept, since services reach each other by name",
                );
            }
            note(
                report,
                at,
                Outcome::Converted,
                format!("listens on {targets:?}"),
            );
            listen.extend(targets);
        }
    }
    service.ports = listen.into_iter().collect();
    Ok(())
}

/// A short port entry: `[ip:][host:]container[/proto]`, or a range of each.
struct PortSpec {
    targets: Vec<u16>,
    published: bool,
    udp: bool,
}

fn short_port(text: &str) -> Result<PortSpec, String> {
    let (rest, proto) = match text.split_once('/') {
        Some((rest, proto)) => (rest, proto),
        None => (text, "tcp"),
    };
    let udp = match proto {
        "tcp" => false,
        "udp" => true,
        other => return Err(format!("port {text}: protocol {other} is not known")),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let published = parts.len() > 1;
    let target = parts.last().copied().unwrap_or(rest);
    let (first, last) = match target.split_once('-') {
        Some((first, last)) => (first, last),
        None => (target, target),
    };
    let first: u16 = first
        .parse()
        .map_err(|_| format!("port {text}: {first} is not a port"))?;
    let last: u16 = last
        .parse()
        .map_err(|_| format!("port {text}: {last} is not a port"))?;
    if first == 0 || last < first || last - first >= 64 {
        return Err(format!("port {text}: a port range of 1 to 64 ports"));
    }
    Ok(PortSpec {
        targets: (first..=last).collect(),
        published,
        udp,
    })
}

/// The service's volumes: named volumes carry over; anonymous volumes, bind mounts and tmpfs mounts are items.
fn volumes(
    name: &str,
    value: Option<&Value>,
    service: &mut Service,
    report: &mut Vec<Item>,
) -> Result<(), String> {
    let Some(list) = value.and_then(Value::as_array) else {
        return Ok(());
    };
    let at = format!("{name}.volumes");
    for entry in list {
        let (source, target, read_only) = match entry {
            Value::String(text) => {
                let parts: Vec<&str> = text.split(':').collect();
                match parts.as_slice() {
                    [target] => (None, *target, false),
                    [source, target] => (Some(*source), *target, false),
                    [source, target, mode] => (Some(*source), *target, *mode == "ro"),
                    _ => {
                        return Err(format!(
                            "service {name}: volume {text} is not source:target"
                        ));
                    }
                }
            }
            Value::Object(long) => {
                let target = long
                    .get("target")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("service {name}: a long volume has no target"))?;
                let read_only = long.get("read_only").and_then(Value::as_bool) == Some(true);
                match long.get("type").and_then(Value::as_str) {
                    Some("volume") => (
                        long.get("source").and_then(Value::as_str),
                        target,
                        read_only,
                    ),
                    kind => {
                        note(
                            report,
                            at.clone(),
                            Outcome::NotConverted,
                            format!(
                                "a {} mount on {target}: the lab VM has no host paths or tmpfs mounts",
                                kind.unwrap_or("bind")
                            ),
                        );
                        continue;
                    }
                }
            }
            _ => {
                return Err(format!(
                    "service {name}: a volume entry is not text or a mapping"
                ));
            }
        };
        let Some(source) = source else {
            note(
                report,
                at.clone(),
                Outcome::NotConverted,
                format!("an anonymous volume on {target}: it has no name for services to share"),
            );
            continue;
        };
        if source.starts_with(['/', '.', '~', '$']) {
            note(
                report,
                at.clone(),
                Outcome::NotConverted,
                format!("a bind mount of {source} on {target}: the lab VM has no host paths"),
            );
            continue;
        }
        let Some(label) = dns_label(source) else {
            note(
                report,
                at.clone(),
                Outcome::NotConverted,
                format!("volume {source} has no DNS-label name"),
            );
            continue;
        };
        if label != source {
            note(
                report,
                at.clone(),
                Outcome::Approximated,
                format!("volume {source} is named {label}"),
            );
        }
        note(
            report,
            at.clone(),
            Outcome::Converted,
            format!("{label} on {target}"),
        );
        service.volumes.push(Volume {
            name: label,
            target: target.to_string(),
            read_only,
        });
    }
    Ok(())
}

/// A volume name as a DNS label: lowercase, underscores as dashes; None when that is still no label.
fn dns_label(name: &str) -> Option<String> {
    let label = name.to_lowercase().replace('_', "-");
    let fits = !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    fits.then_some(label)
}

/// `depends_on` (a list or a map with conditions) and `links`: the services this one waits for.
fn waits(
    name: &str,
    raw: &Map<String, Value>,
    service: &mut Service,
    report: &mut Vec<Item>,
) -> Result<(), String> {
    let mut deps: BTreeSet<String> = BTreeSet::new();
    match raw.get("depends_on") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for item in list {
                let dep = item
                    .as_str()
                    .ok_or_else(|| format!("service {name}: depends_on names a service"))?;
                wait_on(name, dep, "service_started", report, &mut deps);
            }
        }
        Some(Value::Object(map)) => {
            for (dep, options) in map {
                let condition = options
                    .get("condition")
                    .and_then(Value::as_str)
                    .unwrap_or("service_started");
                wait_on(name, dep, condition, report, &mut deps);
            }
        }
        Some(_) => return Err(format!("service {name}: depends_on is a list or a map")),
    }
    if let Some(list) = raw.get("links").and_then(Value::as_array) {
        for item in list {
            let link = item
                .as_str()
                .ok_or_else(|| format!("service {name}: a link is text"))?;
            let (dep, alias) = match link.split_once(':') {
                Some((dep, alias)) => (dep, Some(alias)),
                None => (link, None),
            };
            if let Some(alias) = alias {
                note(
                    report,
                    format!("{name}.links"),
                    Outcome::Approximated,
                    format!("{dep} is reached by its service name; the alias {alias} is not kept"),
                );
            }
            deps.insert(dep.to_string());
        }
    }
    service.depends_on = deps.into_iter().collect();
    Ok(())
}

fn wait_on(
    name: &str,
    dep: &str,
    condition: &str,
    report: &mut Vec<Item>,
    deps: &mut BTreeSet<String>,
) {
    note(
        report,
        format!("{name}.depends_on.{dep}"),
        Outcome::Approximated,
        format!(
            "waits for the ports of {dep} to accept connections (the lab's readiness), not for {condition}"
        ),
    );
    deps.insert(dep.to_string());
}

/// The health check: recorded in the plan (its test and timing); the lab's runner does not run it.
fn healthcheck(
    name: &str,
    value: Option<&Value>,
    service: &mut Service,
    report: &mut Vec<Item>,
) -> Result<(), String> {
    let Some(check) = value.and_then(Value::as_object) else {
        return Ok(());
    };
    let at = format!("{name}.healthcheck");
    if check.get("disable").and_then(Value::as_bool) == Some(true) {
        note(report, at, Outcome::Converted, "disabled: no check");
        return Ok(());
    }
    let test = health_test(
        check
            .get("test")
            .ok_or_else(|| format!("service {name}: healthcheck has no test"))?,
    )
    .map_err(|why| format!("service {name}: healthcheck {why}"))?;
    if test.is_empty() {
        note(report, at, Outcome::Converted, "NONE: no check");
        return Ok(());
    }
    let timing = |key: &str| -> Result<Option<u32>, String> {
        match check.get(key).and_then(Value::as_str) {
            Some(text) => seconds(text).map(Some).ok_or_else(|| {
                format!("service {name}: healthcheck {key} {text} is not a duration")
            }),
            None => Ok(None),
        }
    };
    service.health = Some(Health {
        test,
        interval_secs: timing("interval")?,
        timeout_secs: timing("timeout")?,
        retries: check
            .get("retries")
            .and_then(Value::as_u64)
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
        start_period_secs: timing("start_period")?,
    });
    note(
        report,
        at,
        Outcome::Approximated,
        "recorded in the plan: the lab's runner waits for the ports, and the guest's runtime runs the check (not run on this machine)",
    );
    Ok(())
}

/// The argv of a health check: `CMD` lists its program, `CMD-SHELL` and a string run in a shell; NONE is no check
/// (an empty list).
fn health_test(test: &Value) -> Result<Vec<String>, String> {
    match test {
        Value::String(text) => Ok(vec!["sh".into(), "-c".into(), text.clone()]),
        Value::Array(list) => {
            let words: Vec<String> = list.iter().map(text_of).collect();
            match words.first().map(String::as_str) {
                Some("NONE") => Ok(Vec::new()),
                Some("CMD") => Ok(words[1..].to_vec()),
                Some("CMD-SHELL") => Ok(vec!["sh".into(), "-c".into(), words[1..].join(" ")]),
                _ => Err("test must start with NONE, CMD or CMD-SHELL".into()),
            }
        }
        _ => Err("test is a list or a string".into()),
    }
}

/// A Compose duration (`10s`, `1m30s`, `500ms`) in whole seconds, rounded up.
fn seconds(text: &str) -> Option<u32> {
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    let mut total_ms: u64 = 0;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let amount: u64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let (unit_ms, width) = if rest.starts_with("ms") {
            (1, 2)
        } else if rest.starts_with('s') {
            (1_000, 1)
        } else if rest.starts_with('m') {
            (60_000, 1)
        } else if rest.starts_with('h') {
            (3_600_000, 1)
        } else {
            return None;
        };
        total_ms += amount * unit_ms;
        rest = &rest[width..];
    }
    u32::try_from(total_ms.div_ceil(1_000)).ok()
}

/// `deploy`: the replica count carries over; the resources are the VM's, and the rest is not carried.
fn deploy(
    name: &str,
    value: Option<&Value>,
    service: &mut Service,
    report: &mut Vec<Item>,
) -> Result<(), String> {
    let Some(deploy) = value.and_then(Value::as_object) else {
        return Ok(());
    };
    for (key, value) in deploy {
        match key.as_str() {
            "replicas" => {
                let replicas = value
                    .as_u64()
                    .filter(|n| (1..=u64::from(MAX_REPLICAS)).contains(n))
                    .ok_or_else(|| {
                        format!("service {name}: deploy.replicas is 1 to {MAX_REPLICAS}")
                    })?;
                service.replicas = u32::try_from(replicas).unwrap_or(1);
                note(
                    report,
                    format!("{name}.deploy.replicas"),
                    Outcome::Converted,
                    format!("{replicas} replicas"),
                );
            }
            "resources" => note(
                report,
                format!("{name}.deploy.resources"),
                Outcome::NotConverted,
                "the lab VM is sized for its topology",
            ),
            other => note(
                report,
                format!("{name}.deploy.{other}"),
                Outcome::NotConverted,
                "a deploy setting a topology does not carry",
            ),
        }
    }
    Ok(())
}

/// `${VAR}`, `${VAR:-default}`, `${VAR-default}` and `$VAR` in a value: a default replaces an unset variable (an item,
/// since the lab does not know the owner's variables), a variable without one is an item and is left out, and `$$` is a
/// dollar sign.
fn interpolate(text: &str, at: &str, report: &mut Vec<Item>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at_dollar) = rest.find('$') {
        out.push_str(&rest[..at_dollar]);
        rest = &rest[at_dollar + 1..];
        if let Some(after) = rest.strip_prefix('$') {
            out.push('$');
            rest = after;
        } else if let Some(body) = rest.strip_prefix('{') {
            let Some(end) = body.find('}') else {
                out.push_str("${");
                rest = body;
                continue;
            };
            let inner = &body[..end];
            rest = &body[end + 1..];
            let (var, default) = match inner.split_once(":-") {
                Some((var, default)) => (var, Some(default)),
                None => match inner.split_once('-') {
                    Some((var, default)) => (var, Some(default)),
                    None => (inner, None),
                },
            };
            match default {
                Some(default) => {
                    note(
                        report,
                        at,
                        Outcome::Approximated,
                        format!("{var} is unset in the lab: its default {default:?} is used"),
                    );
                    out.push_str(default);
                }
                None => note(
                    report,
                    at,
                    Outcome::NotConverted,
                    format!("${{{var}}} has no value in the lab: write the value in the topology"),
                ),
            }
        } else {
            let width = rest
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if width == 0 {
                out.push('$');
                continue;
            }
            let var = &rest[..width];
            rest = &rest[width..];
            note(
                report,
                at,
                Outcome::NotConverted,
                format!("${var} has no value in the lab: write the value in the topology"),
            );
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB: &str = include_str!("../tests/fixtures/compose/web-stack.yml");
    const LEGACY: &str = include_str!("../tests/fixtures/compose/unconverted.yml");

    fn service<'a>(topology: &'a Topology, name: &str) -> &'a Service {
        topology.services.iter().find(|s| s.name == name).unwrap()
    }

    fn has(report: &[Item], at: &str, outcome: Outcome) -> bool {
        report
            .iter()
            .any(|item| item.at == at && item.outcome == outcome)
    }

    #[test]
    fn the_web_stack_converts_with_its_wiring_and_nothing_is_lost() {
        let conversion = convert(WEB, None).unwrap();
        let topology = &conversion.topology;
        assert_eq!(topology.name, "web-stack");
        let names: Vec<&str> = topology.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["api", "cache", "db", "web"]);
        let api = service(topology, "api");
        assert_eq!(api.artifact.as_deref(), Some("api"));
        assert_eq!(api.replicas, 2);
        assert_eq!(api.ports, [8081]);
        assert_eq!(api.env["DATABASE_URL"], "postgres://app:app@db:5432/app");
        assert_eq!(api.depends_on, ["cache", "db"]);
        let db = service(topology, "db");
        assert_eq!(db.image.as_deref(), Some("postgres:17"));
        assert_eq!(db.env["POSTGRES_USER"], "app");
        assert_eq!(db.ports, [5432]);
        assert_eq!(
            db.volumes,
            [Volume {
                name: "db-data".into(),
                target: "/var/lib/postgresql/data".into(),
                read_only: false
            }]
        );
        let health = db.health.as_ref().unwrap();
        assert_eq!(health.test, ["sh", "-c", "pg_isready -U app"]);
        assert_eq!(
            (health.interval_secs, health.timeout_secs, health.retries),
            (Some(5), Some(3), Some(5))
        );
        assert_eq!(service(topology, "web").ports, [80]);
        assert_eq!(service(topology, "web").artifact.as_deref(), Some("web"));
        assert!(has(&conversion.report, "api.build", Outcome::Approximated));
        assert!(has(
            &conversion.report,
            "api.environment.DATABASE_URL",
            Outcome::Approximated
        ));
        assert!(has(&conversion.report, "db.volumes", Outcome::Approximated));
        assert!(
            conversion
                .report
                .iter()
                .all(|item| item.outcome != Outcome::NotConverted),
            "{:#?}",
            conversion.report
        );
        let again = crate::spec::to_toml(topology).unwrap();
        assert_eq!(crate::spec::parse(&again).unwrap(), *topology);
    }

    #[test]
    fn what_a_topology_does_not_carry_is_an_item_of_the_report() {
        let conversion = convert(LEGACY, None).unwrap();
        let report = &conversion.report;
        let worker = service(&conversion.topology, "worker");
        // The UDP port is not kept; the published TCP ports are, as listening ports.
        assert_eq!(worker.ports, [9100, 9200]);
        assert_eq!(worker.volumes.len(), 1, "only the named volume is kept");
        assert_eq!(worker.env["TIMEOUT"], "30");
        assert!(!worker.env.contains_key("REGION"));
        for (at, outcome) in [
            ("worker.command", Outcome::NotConverted),
            ("worker.env_file", Outcome::NotConverted),
            ("worker.privileged", Outcome::NotConverted),
            ("worker.network_mode", Outcome::NotConverted),
            ("worker.restart", Outcome::NotConverted),
            ("worker.secrets", Outcome::NotConverted),
            ("worker.labels", Outcome::NotConverted),
            ("worker.ports", Outcome::NotConverted),
            ("worker.ports", Outcome::Approximated),
            ("worker.volumes", Outcome::NotConverted),
            ("worker.deploy.resources", Outcome::NotConverted),
            ("worker.environment.REGION", Outcome::NotConverted),
            ("worker.environment.LOG_LEVEL", Outcome::NotConverted),
            ("worker.environment.TIMEOUT", Outcome::Approximated),
            ("worker.depends_on.cache", Outcome::Approximated),
            ("cache.image", Outcome::Approximated),
            ("cache.profiles", Outcome::Approximated),
            ("cache.healthcheck", Outcome::Converted),
            ("secrets", Outcome::NotConverted),
            ("volumes.scratch.driver", Outcome::NotConverted),
            ("x-note", Outcome::Converted),
        ] {
            assert!(
                has(report, at, outcome),
                "{at} {outcome:?} is not in the report: {report:#?}"
            );
        }
        assert!(
            report
                .iter()
                .any(|i| i.at == "worker.volumes" && i.detail.contains("./data"))
        );
        assert!(report.iter().any(|i| i.detail.contains("UDP")));
        // A healthcheck NONE is no check: nothing to run.
        assert!(service(&conversion.topology, "cache").health.is_none());
    }

    #[test]
    fn a_file_that_cannot_be_a_topology_is_refused_with_why() {
        assert!(
            convert("services:\n  a: {image: y}\n", None)
                .unwrap_err()
                .contains("name")
        );
        assert!(
            convert("name: x\nservices:\n  a: {}\n", None)
                .unwrap_err()
                .contains("neither image nor build")
        );
        assert!(
            convert(
                "name: x\nservices:\n  a: {image: y, depends_on: [b]}\n",
                None
            )
            .unwrap_err()
            .contains("no service of the topology")
        );
        assert!(
            convert(
                "name: x\nservices:\n  a: {image: y, environment: {URL: 'http://ghost:1'}}\n",
                None
            )
            .unwrap_err()
            .contains("ghost")
        );
        assert!(
            convert(
                "name: x\nservices:\n  a: {image: y, ports: ['70000']}\n",
                None
            )
            .unwrap_err()
            .contains("port")
        );
    }

    #[test]
    fn interpolation_takes_defaults_and_reports_what_it_cannot_fill() {
        let mut report = Vec::new();
        assert_eq!(
            interpolate("a$$b ${X:-d} ${Y-e} $Z", "at", &mut report),
            "a$b d e "
        );
        assert_eq!(report.len(), 3);
        assert_eq!(report[2].outcome, Outcome::NotConverted);
    }

    #[test]
    fn durations_are_whole_seconds_rounded_up() {
        assert_eq!(seconds("10s"), Some(10));
        assert_eq!(seconds("1m30s"), Some(90));
        assert_eq!(seconds("500ms"), Some(1));
        assert_eq!(seconds("soon"), None);
    }
}
