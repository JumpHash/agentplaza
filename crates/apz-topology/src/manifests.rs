//! The manifests import (PLAN 22.6): rendered Kubernetes manifests as a services topology. A Helm chart is imported as its
//! `helm template` output; Helm itself is not run here (it is not installed on the machines this runs on), so the rendered
//! YAML is what is read.
//!
//! A Deployment becomes a service: its first container's image (pinned or not), its container ports, its environment
//! (literal values and ConfigMap keys read from the same render), and its replicas. A Service that selects a Deployment
//! names that service: the name the cluster's DNS gives it is the name the topology resolves. Everything else is an item
//! of the report: other kinds, probes, commands, mounts, secrets, and the parts of a manifest a topology has no place for.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;

use crate::report::{Conversion, Item, Outcome, note};
use crate::spec::{Kind, MAX_REPLICAS, Service, Topology};

/// A Deployment as a service, with what the Services that select it need.
struct Built {
    service: Service,
    labels: BTreeMap<String, String>,
    named_ports: BTreeMap<String, u16>,
    deployment: String,
    named_by: Option<String>,
}

/// The rendered manifests in `text`, converted into a topology named `name`.
pub fn convert(text: &str, name: &str) -> Result<Conversion, String> {
    let docs = documents(text)?;
    let mut report = Vec::new();
    let mut deployments = Vec::new();
    let mut services = Vec::new();
    let mut configmaps: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for doc in docs {
        let kind = doc["kind"].as_str().unwrap_or_default().to_string();
        let doc_name = doc["metadata"]["name"].as_str().unwrap_or("?").to_string();
        match kind.as_str() {
            "Deployment" => deployments.push(doc),
            "Service" => services.push(doc),
            "ConfigMap" => {
                if doc.get("binaryData").is_some_and(|b| !is_empty(b)) {
                    note(
                        &mut report,
                        format!("ConfigMap/{doc_name}.binaryData"),
                        Outcome::NotConverted,
                        "binary data is not read",
                    );
                }
                configmaps.insert(doc_name, string_map(&doc["data"]));
            }
            _ => note(
                &mut report,
                format!("{kind}/{doc_name}"),
                Outcome::NotConverted,
                format!("kind {kind} is not converted: Deployment, Service and ConfigMap are"),
            ),
        }
    }

    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut built = Vec::new();
    for doc in &deployments {
        built.push(deployment(doc, &configmaps, &mut used, &mut report)?);
    }
    for doc in &services {
        select(doc, &mut built, &mut report);
    }
    for entry in &built {
        if entry.named_by.is_none() {
            note(
                &mut report,
                format!("Deployment/{}", entry.deployment),
                Outcome::Approximated,
                "no Service selects it: it is named by its Deployment",
            );
        }
    }
    for cm in configmaps.keys() {
        if !used.contains(cm) {
            note(
                &mut report,
                format!("ConfigMap/{cm}"),
                Outcome::NotConverted,
                "no environment reads it: its mounted files are not converted",
            );
        }
    }

    let services: Vec<Service> = built
        .into_iter()
        .map(|entry| {
            let mut service = entry.service;
            service.ports.sort_unstable();
            service.ports.dedup();
            service
        })
        .collect();
    let topology = Topology {
        name: name.to_string(),
        kind: Kind::Services,
        services,
        k3s: None,
        observe: Default::default(),
        traffic: None,
        assertions: Vec::new(),
        resilience: Vec::new(),
        egress: Default::default(),
    };
    topology.check()?;
    Ok(Conversion { topology, report })
}

/// The images a render's Deployments run (containers and init containers), sorted, each once: what a k3s cluster preloads.
pub fn images(text: &str) -> Result<Vec<String>, String> {
    let mut images = BTreeSet::new();
    for doc in documents(text)? {
        if doc["kind"].as_str() != Some("Deployment") {
            continue;
        }
        for list in ["containers", "initContainers"] {
            for container in doc["spec"]["template"]["spec"][list]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(image) = container["image"].as_str() {
                    images.insert(image.to_string());
                }
            }
        }
    }
    Ok(images.into_iter().collect())
}

/// Every document of a multi-document YAML text; a `List` is its items. Empty documents are skipped.
fn documents(text: &str) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for document in serde_yaml_ng::Deserializer::from_str(text) {
        let value: Value = Value::deserialize(document).map_err(|e| format!("manifests: {e}"))?;
        match value {
            Value::Null => {}
            Value::Object(ref map) if map.get("kind").and_then(Value::as_str) == Some("List") => {
                for item in map
                    .get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if item.is_object() {
                        out.push(item.clone());
                    } else {
                        return Err("manifests: a List item is not a mapping".into());
                    }
                }
            }
            Value::Object(_) => out.push(value),
            _ => return Err("manifests: a document is not a mapping".into()),
        }
    }
    Ok(out)
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        Value::Array(list) => list.is_empty(),
        _ => false,
    }
}

/// A mapping of text values (labels, selectors, ConfigMap data); scalars are written as text.
fn string_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| match value {
            Value::String(text) => Some((key.clone(), text.clone())),
            Value::Number(_) | Value::Bool(_) => Some((key.clone(), value.to_string())),
            _ => None,
        })
        .collect()
}

/// A Deployment as a service. The name is the Deployment's until a Service selects it.
fn deployment(
    doc: &Value,
    configmaps: &BTreeMap<String, BTreeMap<String, String>>,
    used: &mut BTreeSet<String>,
    report: &mut Vec<Item>,
) -> Result<Built, String> {
    let name = doc["metadata"]["name"]
        .as_str()
        .ok_or("manifests: a Deployment has no name")?
        .to_string();
    let at = format!("Deployment/{name}");
    let spec = doc["spec"]
        .as_object()
        .ok_or_else(|| format!("{at} has no spec"))?;
    for key in spec.keys() {
        if !matches!(key.as_str(), "replicas" | "selector" | "template") {
            note(
                report,
                format!("{at}.spec.{key}"),
                Outcome::NotConverted,
                "a Deployment setting a topology does not carry",
            );
        }
    }
    let replicas = spec.get("replicas").and_then(Value::as_u64).unwrap_or(1);
    if !(1..=u64::from(MAX_REPLICAS)).contains(&replicas) {
        return Err(format!("{at}: replicas is 1 to {MAX_REPLICAS}"));
    }
    let labels = string_map(&doc["spec"]["template"]["metadata"]["labels"]);
    let pod = doc["spec"]["template"]["spec"]
        .as_object()
        .ok_or_else(|| format!("{at} has no pod spec"))?;
    for (key, value) in pod {
        match key.as_str() {
            "containers" => {}
            "initContainers" if is_empty(value) => {}
            "initContainers" => note(
                report,
                format!("{at}.initContainers"),
                Outcome::NotConverted,
                "init containers are not run",
            ),
            "volumes" if is_empty(value) => {}
            "volumes" => note(
                report,
                format!("{at}.volumes"),
                Outcome::NotConverted,
                "pod volumes are not converted: their mounts are files",
            ),
            other => note(
                report,
                format!("{at}.{other}"),
                Outcome::NotConverted,
                "a pod setting a topology does not carry (placement, security, service accounts)",
            ),
        }
    }
    let containers = pod
        .get("containers")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .ok_or_else(|| format!("{at} has no containers"))?;
    if containers.len() > 1 {
        note(
            report,
            format!("{at}.containers"),
            Outcome::NotConverted,
            format!(
                "only the first container runs as the service; {} sidecar(s) share its pod and are not converted",
                containers.len() - 1
            ),
        );
    }
    let container = &containers[0];
    let mut service = Service {
        name: name.clone(),
        artifact: None,
        image: None,
        replicas: u32::try_from(replicas).unwrap_or(1),
        ports: Vec::new(),
        env: BTreeMap::new(),
        data: None,
        serve: None,
        depends_on: Vec::new(),
        volumes: Vec::new(),
        health: None,
    };
    let mut named_ports = BTreeMap::new();
    let container_at = format!("{at}.containers");
    let image = container["image"]
        .as_str()
        .ok_or_else(|| format!("{at}: its container has no image"))?;
    if !is_pinned(image) {
        note(
            report,
            format!("{container_at}.image"),
            Outcome::Approximated,
            format!("{image} is not pinned to a tag"),
        );
    } else {
        note(
            report,
            format!("{container_at}.image"),
            Outcome::Converted,
            image,
        );
    }
    service.image = Some(image.to_string());
    for port in container["ports"].as_array().into_iter().flatten() {
        let number = port["containerPort"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| format!("{at}: a container port is not a number"))?;
        service.ports.push(number);
        if let Some(port_name) = port["name"].as_str() {
            named_ports.insert(port_name.to_string(), number);
        }
    }
    let mut env = BTreeMap::new();
    for item in container["env"].as_array().into_iter().flatten() {
        let Some(var) = item["name"].as_str() else {
            continue;
        };
        let at = format!("{container_at}.env.{var}");
        if let Some(value) = item["value"].as_str() {
            if value.contains("$(") {
                note(
                    report,
                    at.clone(),
                    Outcome::NotConverted,
                    "refers to another variable with $(...): not expanded",
                );
            }
            env.insert(var.to_string(), value.to_string());
        } else if let Some(source) = item.get("valueFrom") {
            if let Some(key_ref) = source.get("configMapKeyRef") {
                let cm = key_ref["name"].as_str().unwrap_or_default();
                let key = key_ref["key"].as_str().unwrap_or_default();
                used.insert(cm.to_string());
                match configmaps.get(cm).and_then(|data| data.get(key)) {
                    Some(value) => {
                        env.insert(var.to_string(), value.clone());
                    }
                    None if key_ref["optional"].as_bool() == Some(true) => {}
                    None => note(
                        report,
                        at,
                        Outcome::NotConverted,
                        format!("ConfigMap {cm} has no key {key} in the render"),
                    ),
                }
            } else if source.get("secretKeyRef").is_some() {
                note(
                    report,
                    at,
                    Outcome::NotConverted,
                    "a Secret's value: secrets are not in a topology",
                );
            } else {
                note(
                    report,
                    at,
                    Outcome::NotConverted,
                    "the pod's own fields are not converted",
                );
            }
        } else {
            env.insert(var.to_string(), String::new());
        }
    }
    for item in container["envFrom"].as_array().into_iter().flatten() {
        let prefix = item["prefix"].as_str().unwrap_or_default();
        if let Some(map_ref) = item.get("configMapRef") {
            let cm = map_ref["name"].as_str().unwrap_or_default();
            used.insert(cm.to_string());
            match configmaps.get(cm) {
                Some(data) => {
                    for (key, value) in data {
                        env.insert(format!("{prefix}{key}"), value.clone());
                    }
                }
                None if map_ref["optional"].as_bool() == Some(true) => {}
                None => note(
                    report,
                    format!("{container_at}.envFrom"),
                    Outcome::NotConverted,
                    format!("ConfigMap {cm} is not in the render"),
                ),
            }
        } else if item.get("secretRef").is_some() {
            note(
                report,
                format!("{container_at}.envFrom"),
                Outcome::NotConverted,
                "a Secret's values: secrets are not in a topology",
            );
        }
    }
    service.env = env;
    container_keys(&container_at, container, report);
    Ok(Built {
        service,
        labels,
        named_ports,
        deployment: name,
        named_by: None,
    })
}

/// The container's own keys the import does not carry, each an item.
fn container_keys(at: &str, container: &Value, report: &mut Vec<Item>) {
    let Some(map) = container.as_object() else {
        return;
    };
    for key in map.keys() {
        let why = match key.as_str() {
            "name"
            | "image"
            | "ports"
            | "env"
            | "envFrom"
            | "imagePullPolicy"
            | "terminationMessagePath"
            | "terminationMessagePolicy" => continue,
            "command" | "args" => "the lab starts the artifact or the image's own program",
            "livenessProbe" | "readinessProbe" | "startupProbe" => {
                "probes are not converted: the runner waits for the ports"
            }
            "resources" => "the lab VM is sized for its topology",
            "volumeMounts" => "mounts are not converted",
            _ => "a container setting a topology does not carry",
        };
        note(report, format!("{at}.{key}"), Outcome::NotConverted, why);
    }
}

/// A Service: the Deployments its selector matches are named by it, and its ports are the ones the topology listens on.
fn select(doc: &Value, built: &mut [Built], report: &mut Vec<Item>) {
    let name = doc["metadata"]["name"].as_str().unwrap_or("?").to_string();
    let at = format!("Service/{name}");
    let selector = string_map(&doc["spec"]["selector"]);
    if selector.is_empty() {
        note(
            report,
            at,
            Outcome::NotConverted,
            "no selector: it selects no Deployment",
        );
        return;
    }
    let matches: Vec<usize> = built
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            selector
                .iter()
                .all(|(key, value)| entry.labels.get(key) == Some(value))
        })
        .map(|(index, _)| index)
        .collect();
    let Some(&first) = matches.first() else {
        note(
            report,
            at,
            Outcome::NotConverted,
            "its selector matches no Deployment in the render",
        );
        return;
    };
    if matches.len() > 1 {
        note(
            report,
            at.clone(),
            Outcome::NotConverted,
            format!(
                "selects {} Deployments: the first is named {name}, the others are not",
                matches.len()
            ),
        );
    }
    let entry = &mut built[first];
    if let Some(other) = &entry.named_by {
        note(
            report,
            at,
            Outcome::NotConverted,
            format!(
                "its Deployment {} is already named by {other}",
                entry.deployment
            ),
        );
        return;
    }
    entry.service.name.clone_from(&name);
    entry.named_by = Some(name.clone());
    let kind = doc["spec"]["type"].as_str().unwrap_or("ClusterIP");
    if kind != "ClusterIP" {
        note(
            report,
            at.clone(),
            Outcome::Approximated,
            format!(
                "type {kind} is not published outside the cluster: the lab has no inbound map here"
            ),
        );
    }
    for port in doc["spec"]["ports"].as_array().into_iter().flatten() {
        let served = port["port"].as_u64();
        let target = match &port["targetPort"] {
            Value::Number(n) => n.as_u64(),
            Value::String(port_name) => entry.named_ports.get(port_name).map(|n| u64::from(*n)),
            _ => served,
        };
        let Some(target) = target.and_then(|t| u16::try_from(t).ok()) else {
            note(
                report,
                at.clone(),
                Outcome::NotConverted,
                format!(
                    "a targetPort {} names no container port",
                    port["targetPort"]
                ),
            );
            continue;
        };
        if served != Some(u64::from(target)) {
            note(
                report,
                at.clone(),
                Outcome::Approximated,
                format!(
                    "port {} is served on {target} in the lab: consumers reach it there",
                    served.unwrap_or_default()
                ),
            );
        }
        entry.service.ports.push(target);
    }
}

/// A tag-pinned or digest-pinned image.
fn is_pinned(image: &str) -> bool {
    if image.contains('@') {
        return true;
    }
    let last = image.rsplit('/').next().unwrap_or(image);
    matches!(last.split_once(':'), Some((_, tag)) if !tag.is_empty() && tag != "latest")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_and_empty_documents_are_read() {
        let text = "---\n# Source: chart/templates/a.yaml\nkind: List\nitems:\n- kind: ConfigMap\n  metadata: {name: c}\n  data: {A: b}\n---\n";
        let docs = documents(text).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["kind"], "ConfigMap");
        assert!(documents("- just\n- a list\n").is_err());
    }

    #[test]
    fn images_are_the_deployments_containers_once_each() {
        let text = "kind: Deployment\nmetadata: {name: a}\nspec:\n  template:\n    spec:\n      containers:\n      - {name: x, image: 'redis:7'}\n      initContainers:\n      - {name: y, image: 'busybox:1.36'}\n---\nkind: Deployment\nmetadata: {name: b}\nspec:\n  template:\n    spec:\n      containers:\n      - {name: z, image: 'redis:7'}\n";
        assert_eq!(images(text).unwrap(), ["busybox:1.36", "redis:7"]);
    }
}

#[cfg(test)]
mod rendered_tests {
    use super::*;

    const RENDER: &str = include_str!("../tests/fixtures/manifests/web-stack.yaml");

    fn service<'a>(topology: &'a Topology, name: &str) -> &'a Service {
        topology.services.iter().find(|s| s.name == name).unwrap()
    }

    #[test]
    fn a_rendered_chart_converts_its_deployments_named_by_their_services() {
        let conversion = convert(RENDER, "web-stack").unwrap();
        let topology = &conversion.topology;
        let names: Vec<&str> = topology.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["api", "db", "cache"]);
        let api = service(topology, "api");
        assert_eq!(api.image.as_deref(), Some("registry.example.com/api:1.4.2"));
        assert_eq!(api.replicas, 2);
        assert_eq!(api.ports, [8080]);
        assert_eq!(api.env["LOG_LEVEL"], "info");
        assert_eq!(api.env["CACHE_URL"], "redis://cache:6379");
        assert_eq!(api.env["DATABASE_URL"], "postgres://app@db/app");
        assert!(!api.env.contains_key("DB_PASSWORD"));
        let report = &conversion.report;
        let has =
            |at: &str, outcome: Outcome| report.iter().any(|i| i.at == at && i.outcome == outcome);
        assert!(has("Service/api", Outcome::Approximated), "{report:#?}");
        assert!(has(
            "Deployment/web-stack-api.containers.resources",
            Outcome::NotConverted
        ));
        assert!(has(
            "Deployment/web-stack-api.containers.readinessProbe",
            Outcome::NotConverted
        ));
        assert!(has(
            "Deployment/web-stack-api.containers.env.DB_PASSWORD",
            Outcome::NotConverted
        ));
        assert!(has(
            "Deployment/web-stack-cache.containers.image",
            Outcome::Approximated
        ));
        assert!(has("Service/cache-ext", Outcome::NotConverted));
        assert!(has("Job/migrate", Outcome::NotConverted));
    }

    #[test]
    fn the_render_names_its_images_once_each() {
        assert_eq!(
            images(RENDER).unwrap(),
            ["postgres:17", "redis", "registry.example.com/api:1.4.2"]
        );
    }

    #[test]
    fn a_manifest_with_an_unknown_reference_still_converts_and_says_so() {
        let text = "kind: Deployment\nmetadata: {name: solo}\nspec:\n  template:\n    spec:\n      containers:\n      - name: solo\n        image: 'x:1'\n        env:\n        - name: MISSING\n          valueFrom: {configMapKeyRef: {name: nowhere, key: k}}\n";
        let conversion = convert(text, "solo").unwrap();
        assert!(
            conversion
                .report
                .iter()
                .any(|i| i.at == "Deployment/solo.containers.env.MISSING"
                    && i.outcome == Outcome::NotConverted)
        );
        assert_eq!(conversion.topology.services[0].name, "solo");
    }
}
