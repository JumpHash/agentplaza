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

fn mac_node() -> NodeChoice {
    node("n3", "mini", &["plaza"], true, true)
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

fn problem(form: &VmForm, node: Option<&NodeChoice>, macos: usize) -> String {
    vm_problem(form, node, macos).unwrap_or_default()
}

#[test]
fn a_vm_name_is_a_dns_label() {
    let longest = "a".repeat(63);
    let too_long = "a".repeat(64);
    for ok in ["desk-1", "a", "x-1", " desk-2 ", longest.as_str()] {
        assert_eq!(vm_name_problem(ok), None, "{ok:?}");
    }
    for bad in [
        "",
        "  ",
        "Desk",
        "-a",
        "a-",
        "a_b",
        "a b",
        too_long.as_str(),
    ] {
        assert!(vm_name_problem(bad).is_some(), "{bad:?}");
    }
}

#[test]
fn whole_numbers_are_read_within_their_range_only() {
    assert_eq!(whole_in("16", VM_CPUS), Some(16));
    assert_eq!(whole_in(" 4 ", VM_CPUS), Some(4));
    assert_eq!(whole_in("17", VM_CPUS), None);
    assert_eq!(whole_in("", VM_CPUS), None);
    assert_eq!(whole_in("1.5", VM_CPUS), None);
}

#[test]
fn a_passthrough_address_and_a_windows_image_have_their_shape() {
    assert_eq!(pci_problem("0000:01:00.0"), None);
    assert_eq!(pci_problem("0000:0A:00.7"), None);
    assert!(pci_problem("01:00.0").is_some());
    assert!(pci_problem("0000:01:00.9").is_some());
    assert_eq!(image_problem("/srv/win.img"), None);
    assert_eq!(image_problem("D:\\images\\win.img"), None);
    assert!(image_problem("win.img").is_some());
    assert!(image_problem("  ").is_some());
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
fn a_node_option_names_its_backends_and_qemu() {
    assert_eq!(
        qemu_node().to_string(),
        "tower (n2) · plaza, qemu · QEMU/OVMF"
    );
    let older = NodeChoice {
        reported: false,
        backends: Vec::new(),
        online: false,
        ..plaza_node()
    };
    assert_eq!(
        older.to_string(),
        "studio (n1) · backends not reported · no QEMU/OVMF · offline"
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
fn the_form_asks_for_a_node_then_a_name_then_the_sizes() {
    assert_eq!(
        problem(&linux_form(), None, 0),
        "Choose a node to run the VM on."
    );
    let unnamed = VmForm {
        name: String::new(),
        ..linux_form()
    };
    assert_eq!(
        problem(&unnamed, Some(&plaza_node()), 0),
        "Give the VM a name."
    );
    let few_cpus = VmForm {
        cpus: "0".to_string(),
        ..linux_form()
    };
    assert!(problem(&few_cpus, Some(&plaza_node()), 0).contains("CPUs"));
    let many_cpus = VmForm {
        cpus: "17".to_string(),
        ..linux_form()
    };
    assert!(problem(&many_cpus, Some(&plaza_node()), 0).contains("CPUs"));
    let small_ram = VmForm {
        ram_mb: "1024".to_string(),
        ..linux_form()
    };
    assert!(problem(&small_ram, Some(&plaza_node()), 0).contains("Memory"));
    let small_disk = VmForm {
        disk_gb: "7".to_string(),
        ..linux_form()
    };
    assert!(problem(&small_disk, Some(&plaza_node()), 0).contains("Disk"));
    assert_eq!(
        problem(&linux_form(), Some(&plaza_node()), 0),
        "",
        "a linux VM with the defaults is fine"
    );
}

#[test]
fn a_windows_vm_needs_an_image_and_more_memory() {
    let no_image = VmForm {
        image: String::new(),
        ..windows_form()
    };
    assert!(problem(&no_image, Some(&qemu_node()), 0).contains("needs the path"));
    let relative = VmForm {
        image: "win.img".to_string(),
        ..windows_form()
    };
    assert!(problem(&relative, Some(&qemu_node()), 0).contains("absolute"));
    let low_ram = VmForm {
        ram_mb: "2048".to_string(),
        ..windows_form()
    };
    assert!(problem(&low_ram, Some(&qemu_node()), 0).contains("4096"));
    assert_eq!(problem(&windows_form(), Some(&qemu_node()), 0), "");
}

#[test]
fn a_passthrough_gpu_needs_its_address_and_qemu() {
    let no_pci = VmForm {
        gpu: GpuChoice::Vfio,
        pci: "01:00.0".to_string(),
        ..linux_form()
    };
    assert!(problem(&no_pci, Some(&qemu_node()), 0).contains("PCI"));
    let good = VmForm {
        gpu: GpuChoice::Vfio,
        pci: "0000:01:00.0".to_string(),
        ..linux_form()
    };
    assert!(problem(&good, Some(&plaza_node()), 0).contains("no QEMU/OVMF"));
    assert_eq!(problem(&good, Some(&qemu_node()), 0), "");
}

#[test]
fn a_macos_vm_runs_only_on_apple_hardware_and_at_most_two_per_mac() {
    let mac = VmForm {
        os: Os::MacOs,
        ..linux_form()
    };
    assert!(problem(&mac, Some(&plaza_node()), 0).contains("Apple hardware"));
    assert_eq!(problem(&mac, Some(&mac_node()), 1), "");
    assert!(problem(&mac, Some(&mac_node()), 2).contains("At most 2"));
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
fn the_backend_hint_says_where_the_vm_runs_and_why() {
    assert_eq!(
        backend_hint(&linux_form(), false),
        "Runs on plaza: auto picks plaza for this guest."
    );
    assert_eq!(
        backend_hint(&windows_form(), true),
        "Runs on QEMU: auto picks QEMU for a Windows VM."
    );
    let plaza_windows = VmForm {
        backend: BackendChoice::Plaza,
        ..windows_form()
    };
    assert!(backend_hint(&plaza_windows, true).contains("cannot run a Windows VM"));
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
fn a_macos_count_is_read_from_the_vms_on_that_node() {
    let vms = Load::Ready(vec![
        Vm {
            id: "v1".into(),
            node: "n3".into(),
            os: "macos".into(),
            ..Vm::default()
        },
        Vm {
            id: "v2".into(),
            node: "n2".into(),
            os: "macos".into(),
            ..Vm::default()
        },
        Vm {
            id: "v3".into(),
            node: "n3".into(),
            os: "linux".into(),
            ..Vm::default()
        },
    ]);
    assert_eq!(macos_on(&vms, "n3"), 1);
    assert_eq!(macos_on(&Load::<Vec<Vm>>::Pending, "n3"), 0);
}

#[test]
fn a_vm_state_is_online_then_offline_once_enrolled_then_joining() {
    let base = Vm::default();
    assert_eq!(
        vm_state_chip(&Vm {
            online: true,
            station: Some("w1".into()),
            ..base.clone()
        }),
        ("online".to_string(), ui::Tone::Ok)
    );
    assert_eq!(
        vm_state_chip(&Vm {
            station: Some("w1".into()),
            ..base.clone()
        }),
        ("offline".to_string(), ui::Tone::Muted)
    );
    assert_eq!(
        vm_state_chip(&base),
        ("joining".to_string(), ui::Tone::Info)
    );
}

#[test]
fn a_vm_reads_in_words() {
    let vm = Vm {
        cpus: 4,
        ram_mb: 16384,
        disk_gb: 80,
        ..Vm::default()
    };
    assert_eq!(vm_size(&vm), "4 vCPU · 16 GB RAM · 80 GB disk");
    assert_eq!(vm_gpu("none"), "no GPU");
    assert_eq!(vm_gpu("virtual"), "virtual GPU");
    assert_eq!(vm_gpu("vfio:0000:01:00.0"), "passthrough 0000:01:00.0");
    assert_eq!(Os::name_of("macos"), "macOS");
    assert_eq!(Os::name_of("plan9"), "plan9");
    assert_eq!(node_facts_line(None), "Choose the node that runs the VM.");
    assert_eq!(
        node_facts_line(Some(&qemu_node())),
        "Backends: plaza, qemu · QEMU/OVMF: yes · Apple hardware: no"
    );
}

#[test]
fn an_edit_sets_its_field() {
    let mut form = VmForm::default();
    apply_edit(&mut form, VmFieldEdit::Name("desk-9".to_string()));
    apply_edit(&mut form, VmFieldEdit::Gpu(GpuChoice::Vfio));
    apply_edit(&mut form, VmFieldEdit::Backend(BackendChoice::Qemu));
    apply_edit(&mut form, VmFieldEdit::Os(Os::Windows));
    assert_eq!(form.name, "desk-9");
    assert_eq!(form.gpu, GpuChoice::Vfio);
    assert_eq!(form.backend, BackendChoice::Qemu);
    assert_eq!(form.os, Os::Windows);
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
