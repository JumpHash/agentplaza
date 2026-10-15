use super::*;
use serde_json::json;

fn node(id: &str, name: &str, backends: &[&str], apple: bool, online: bool) -> NodeChoice {
    let backends: Vec<String> = backends.iter().map(|b| b.to_string()).collect();
    NodeChoice {
        id: id.to_string(),
        name: name.to_string(),
        reported: true,
        qemu: backends.iter().any(|b| b == "qemu"),
        backends,
        apple,
        online,
    }
}

fn plaza_node() -> NodeChoice {
    node("n1", "studio", &["plaza"], false, true)
}

fn qemu_node() -> NodeChoice {
    node("n2", "tower", &["plaza", "qemu"], false, true)
}

fn linux_form() -> VmForm {
    VmForm {
        node: "n1".to_string(),
        name: "desk-1".to_string(),
        ..VmForm::default()
    }
}

fn windows_form() -> VmForm {
    VmForm {
        node: "n2".to_string(),
        os: Os::Windows,
        ram_mb: "8192".to_string(),
        image: "D:\\images\\win.img".to_string(),
        ..linux_form()
    }
}

#[test]
fn a_node_is_read_the_way_the_hub_reads_it() {
    let facts: NodeFacts = serde_json::from_value(json!({
        "id": "n2", "name": "tower", "revoked": false, "online": true,
        "machine": {"cpus": 8, "workstation_backends": ["plaza", "qemu"]}
    }))
    .unwrap();
    let apple: NodeFacts = serde_json::from_value(json!({
        "id": "n3", "name": "", "revoked": false, "online": false,
        "machine": {"vendor": "apple", "apple": {"chip": "M4"}, "workstation_backends": ["plaza"]}
    }))
    .unwrap();
    let older: NodeFacts =
        serde_json::from_value(json!({"id": "n4", "machine": {"cpus": 4}})).unwrap();
    let null: NodeFacts = serde_json::from_value(json!({"id": "n5", "machine": null})).unwrap();
    let revoked: NodeFacts = serde_json::from_value(json!({"id": "n6", "revoked": true})).unwrap();

    let choices = node_choices(&[facts, apple, older, null, revoked]);
    let ids: Vec<&str> = choices.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        ["n2", "n3", "n4", "n5"],
        "a revoked node is not offered"
    );
    assert_eq!(
        choices[0],
        node("n2", "tower", &["plaza", "qemu"], false, true)
    );
    assert!(choices[1].apple && !choices[1].qemu && !choices[1].online);
    assert_eq!(choices[1].name, "n3", "a node without a name shows its id");
    assert!(
        !choices[2].reported && choices[2].backends.is_empty(),
        "an older node reports no backends"
    );
    assert!(
        !choices[3].reported,
        "a node with no machine summary reports no backends"
    );
}

#[test]
fn a_vm_as_the_hub_lists_it_reads_and_ignores_the_join_code() {
    let vm: Vm = serde_json::from_value(json!({
        "id": "v3", "node": "n2", "vm": "wv3", "name": "desk-1", "os": "windows", "cpus": 4, "ram_mb": 8192,
        "disk_gb": 40, "gpu": "vfio:0000:01:00.0", "image": "D:\\images\\win.img", "backend": "qemu",
        "code": "j9", "station": "w4", "created_ms": 1, "online": true
    }))
    .unwrap();
    assert_eq!(vm.name, "desk-1");
    assert_eq!(vm.station.as_deref(), Some("w4"));
    assert!(vm.online);
}

#[test]
fn the_backend_is_qemu_for_windows_and_a_passthrough_gpu_and_plaza_otherwise() {
    let auto = |os: Os, gpu: GpuChoice| VmForm {
        os,
        gpu,
        ..linux_form()
    };
    assert_eq!(
        backend_for(&auto(Os::Linux, GpuChoice::None), false),
        Ok(Launch::Plaza)
    );
    assert_eq!(
        backend_for(&auto(Os::Windows, GpuChoice::None), true),
        Ok(Launch::Qemu)
    );
    assert_eq!(
        backend_for(&auto(Os::Linux, GpuChoice::Vfio), true),
        Ok(Launch::Qemu)
    );
    assert_eq!(
        backend_for(&auto(Os::Windows, GpuChoice::None), false),
        Err("This node has no QEMU/OVMF; install qemu and edk2-ovmf.".to_string())
    );
    let plaza = VmForm {
        backend: BackendChoice::Plaza,
        ..auto(Os::Windows, GpuChoice::None)
    };
    assert_eq!(backend_for(&plaza, true), Ok(Launch::Plaza));
    let qemu_mac = VmForm {
        backend: BackendChoice::Qemu,
        ..auto(Os::MacOs, GpuChoice::None)
    };
    assert!(
        backend_for(&qemu_mac, true).is_err(),
        "a macOS VM is never QEMU"
    );
}

#[test]
fn the_body_names_only_what_applies() {
    assert_eq!(
        read_vm_form(&linux_form(), Some(&plaza_node()), 0),
        Ok(json!({
            "node": "n1", "os": "linux", "name": "desk-1", "cpus": 4, "ram_mb": 8192, "disk_gb": 40,
            "gpu": "none", "backend": "auto"
        }))
    );
    let body = read_vm_form(&windows_form(), Some(&qemu_node()), 0).unwrap();
    assert_eq!(body["os"], "windows");
    assert_eq!(body["image"], "D:\\images\\win.img");
    let linux_with_image = VmForm {
        image: "/srv/win.img".to_string(),
        ..linux_form()
    };
    let body = read_vm_form(&linux_with_image, Some(&plaza_node()), 0).unwrap();
    assert!(
        body.get("image").is_none(),
        "an image goes only with a Windows VM"
    );
    assert_eq!(
        read_vm_form(&linux_form(), None, 0),
        Err("Choose a node to run the VM on.".to_string()),
        "a form with a problem builds no body"
    );
}

#[test]
fn a_passthrough_gpu_is_sent_as_vfio_with_its_address_and_the_name_is_trimmed() {
    let form = VmForm {
        name: "  desk-1  ".to_string(),
        gpu: GpuChoice::Vfio,
        pci: " 0000:01:00.0 ".to_string(),
        ..linux_form()
    };
    let body = read_vm_form(&form, Some(&qemu_node()), 0).unwrap();
    assert_eq!(body["gpu"], "vfio:0000:01:00.0");
    assert_eq!(body["name"], "desk-1");
    assert_eq!(body["backend"], "auto");
}

#[test]
fn sleep_and_wake_reach_only_plaza_vms() {
    let vm = |backend: &str| Vm {
        backend: backend.to_string(),
        ..Vm::default()
    };
    assert!(runs_on_plaza(&vm("plaza")));
    assert!(runs_on_plaza(&vm("")));
    assert!(!runs_on_plaza(&vm("qemu")));
}
