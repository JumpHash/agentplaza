//! Headless rendering: the register laid out and drawn for real, without a
//! window.
//!
//! iced's test harness drives the very same widgets and application the binary
//! runs, so these tests are the runtime verification for a host where a window
//! cannot be looked at: they assert what the sheet says, and what a click on
//! it produces, against a stub daemon over a real socket.

mod common;

use std::time::Duration;

use common::{DETAIL, EXITS, LIST, MACHINE, REPORT_DETAIL, ROLE_DETAIL, ROLE_LIST};

use agentplaza_desktop::api::{Client, Endpoint};
use agentplaza_desktop::cli::Config;
use agentplaza_desktop::export::ExportTarget;
use agentplaza_desktop::model::{
    Effect, Message, Model, NoticeKind, Pane, Selection, DEFAULT_REFRESH_MS,
};
use agentplaza_desktop::{app, theme, view};

use iced::Size;
use iced_test::selector::Candidate;
use iced_test::simulator;

/// The page, as RGBA.
const PAPER: [u8; 4] = [0xe9, 0xec, 0xe5, 0xff];
/// The ink.
const INK: [u8; 4] = [0x15, 0x19, 0x16, 0xff];
/// The green a running state is stamped in.
const OK: [u8; 4] = [0x2e, 0x6a, 0x40, 0xff];
/// The amber a killed state is stamped in.
const WARN: [u8; 4] = [0x85, 0x64, 0x0e, 0xff];
/// The red the map draws a killed VM's mark in.
const CRIT: [u8; 4] = [0xa3, 0x18, 0x2b, 0xff];
/// The machine's substrate region on the map, and nothing else.
const SUBSTRATE: [u8; 4] = [0xee, 0xf1, 0xe9, 0xff];
/// The one hot signal: the map's outline around the selected cell.
const SIGNAL: [u8; 4] = [0xe8, 0x59, 0x0c, 0xff];
/// A sheet laid on the page: the help box, and every ruled sheet behind it.
const SHEET: [u8; 4] = [0xf4, 0xf6, 0xf1, 0xff];

/// The daemon stub's configuration.
fn config(port: u16) -> Config {
    Config {
        endpoint: Endpoint::loopback_at("127.0.0.1", port).expect("loopback"),
        timeout: Duration::from_secs(5),
        refresh_ms: DEFAULT_REFRESH_MS,
    }
}

/// A model holding whatever the stub's list endpoint answers, read through the
/// real client.
fn model_from(port: u16) -> Model {
    let mut model = Model::at(Endpoint::loopback_at("127.0.0.1", port).expect("loopback"));
    let page = Client::new(model.endpoint.clone())
        .runs()
        .expect("the stub answers the list");
    model.apply(Message::Runs {
        at_ms: 1_760_000_500_000,
        result: Ok(page),
    });
    model
}

/// The same, with the machine endpoint's answer in it too.
fn machine_from(port: u16) -> Model {
    let mut model = model_from(port);
    let machine = Client::new(model.endpoint.clone())
        .machine()
        .expect("the stub answers the machine");
    model.apply(Message::Machine {
        at_ms: 1_760_000_500_000,
        result: Ok(machine),
    });
    model
}

/// A model that has read the machine and nothing else: no list poll has
/// answered, so the session has not observed anything yet.
fn machine_only(port: u16) -> Model {
    let mut model = Model::at(Endpoint::loopback_at("127.0.0.1", port).expect("loopback"));
    let machine = Client::new(model.endpoint.clone())
        .machine()
        .expect("the stub answers the machine");
    model.apply(Message::Machine {
        at_ms: 1_760_000_500_000,
        result: Ok(machine),
    });
    model
}

/// Does what the runtime does for an effect, synchronously: the calls a click
/// asked for, against the same client the application uses.
fn perform(effect: Effect, model: &mut Model, client: &Client) {
    if effect.poll_list {
        let result = client.runs();
        model.apply(Message::Runs {
            at_ms: model.now_ms,
            result,
        });
    }
    if effect.poll_machine {
        let result = client.machine();
        model.apply(Message::Machine {
            at_ms: model.now_ms,
            result,
        });
    }
    if let Some(id) = effect.fetch_run {
        let result = client.run(&id);
        model.apply(Message::Run {
            at_ms: model.now_ms,
            id,
            result,
        });
    }
    for id in effect.fetch_map {
        let result = client.run(&id);
        model.apply(Message::Run {
            at_ms: model.now_ms,
            id,
            result,
        });
    }
    if let Some(id) = effect.kill {
        let result = client.kill(&id);
        model.apply(Message::Killed {
            at_ms: model.now_ms,
            id,
            result,
        });
    }
    if let Some(create) = effect.spawn {
        let result = client.spawn(&create);
        model.apply(Message::Spawned {
            at_ms: model.now_ms,
            result,
        });
    }
    if let Some(call) = effect.input {
        let result = client.input(&call.run, call.vm, &call.data);
        model.apply(Message::Input {
            at_ms: model.now_ms,
            id: call.run,
            vm: call.vm,
            result,
        });
    }
    if let Some(call) = effect.export {
        // What the runtime does for an export: the register's own write, not
        // a request to the daemon.
        let result = agentplaza_desktop::export::write_export(&call.path, &call.contents);
        model.apply(Message::Exported {
            at_ms: model.now_ms,
            target: call.target,
            run: call.run,
            vm: call.vm,
            path: call.path,
            result,
        });
    }
    if let Some(call) = effect.csv {
        // The session CSV goes through the same one write.
        let result = agentplaza_desktop::export::write_export(&call.path, &call.contents);
        model.apply(Message::CsvExported {
            at_ms: model.now_ms,
            path: call.path,
            result,
        });
    }
}

/// The register's view, driven from a fixed model: the frame the harness
/// renders for a pane that no click can reach (the map's cells are canvas
/// geometry, not widgets).
fn pane_program(model: Model) -> impl iced::Program<State = Model, Message = Message, Theme = iced::Theme>
{
    iced::application(
        move || (model.clone(), iced::Task::none()),
        |state: &mut Model, message: Message| {
            let _ = state.apply(message);
            iced::Task::none()
        },
        view::view,
    )
    .title("AgentPlaza register — fleet")
    .theme(|_: &Model| theme::theme())
    .window_size([1280.0, 860.0])
}

/// Renders `model`'s pane through the register's own view.
fn frame_of(model: Model) -> iced::window::Screenshot {
    iced_test::screenshot(
        &pane_program(model),
        &theme::theme(),
        Size::new(1280.0, 860.0),
        1.0,
        Duration::from_millis(200),
    )
}

#[test]
fn the_register_renders_the_fleets_runs() {
    let server = common::fleet(LIST, DETAIL);
    let model = model_from(server.port);
    let mut ui = simulator(view::view(&model));

    for expected in [
        "AgentPlaza register",
        "RUNNING",
        "KILLED",
        "2/2 up",
        "50+",
        "LIVE",
        "512 MiB",
    ] {
        assert!(
            ui.find(expected).is_ok(),
            "the register should show {expected:?}"
        );
    }
}

#[test]
fn clicking_a_run_opens_it_and_renders_its_serial() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    let client = Client::new(model.endpoint.clone());

    // The click the operator makes on the register.
    let mut ui = simulator(view::view(&model));
    ui.click("r1").expect("the run's entry is clickable");
    let messages: Vec<Message> = ui.into_messages().collect();
    assert!(
        messages.contains(&Message::Show(Pane::Run("r1".to_string()))),
        "clicking the entry asks for the run, got {messages:?}"
    );

    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }
    assert_eq!(model.pane, Pane::Run("r1".to_string()));
    assert_eq!(model.detail.as_ref().expect("the run").id, "r1");

    // What the sheet now says.
    let mut ui = simulator(view::view(&model));
    for expected in [
        "Linux plaza 7.2.8 x86_64",
        "login: plaza",
        "cid 50",
        "FOLLOW ON",
        "KILL",
    ] {
        assert!(
            ui.find(expected).is_ok(),
            "the run pane should show {expected:?}"
        );
    }
}

#[test]
fn clicking_new_run_opens_the_spawn_form() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);

    let mut ui = simulator(view::view(&model));
    ui.click("New run").expect("the entry is clickable");
    for message in ui.into_messages() {
        let effect = model.apply(message);
        assert!(
            effect.is_idle(),
            "opening the form needs no daemon call, got {effect:?}"
        );
    }
    assert_eq!(model.pane, Pane::Spawn);

    let mut ui = simulator(view::view(&model));
    for expected in ["vsock_cid", "memory_mib", "BOOT RUN", "1–64 VMs"] {
        assert!(
            ui.find(expected).is_ok(),
            "the spawn form should show {expected:?}"
        );
    }
}

#[test]
fn an_invalid_form_draws_its_refusal() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    model.pane = Pane::Spawn;
    model.apply(Message::Edit {
        field: agentplaza_desktop::form::Field::MemoryMib,
        value: "16".to_string(),
    });
    model.apply(Message::SpawnClicked);

    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find("memory_mib must be at least 128").is_ok(),
        "the field's complaint is on the sheet"
    );
    assert!(ui.find("REFUSED").is_ok());
}

#[test]
fn the_whole_application_draws_a_frame_of_the_live_fleet() {
    let server = common::fleet(LIST, DETAIL);
    let program = app::application(config(server.port));
    let frame = iced_test::screenshot(
        &program,
        &theme::theme(),
        Size::new(1280.0, 860.0),
        1.0,
        Duration::from_millis(1_200),
    );

    assert_eq!((frame.size.width, frame.size.height), (1280, 860));
    let pixels: &[u8] = frame.rgba.as_ref();
    assert_eq!(pixels.len(), 1280 * 860 * 4);

    for (name, color, tolerance) in [
        ("the page", PAPER, 2),
        ("the ink", INK, 8),
        ("a running stamp", OK, 8),
        ("a killed stamp", WARN, 8),
    ] {
        assert!(
            pixels.chunks_exact(4).any(|pixel| near(pixel, color, tolerance)),
            "{name} is not on the frame"
        );
    }

    let distinct = distinct_colors(pixels);
    assert!(
        distinct > 100,
        "a register with data has many shades, got {distinct}"
    );

    // Leave the frame where a human can look at it.
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("fleet-frame.rgba");
    std::fs::write(&path, pixels).expect("the frame should be written");
    println!(
        "frame: {}x{}, {} distinct colors, raw RGBA at {}",
        frame.size.width,
        frame.size.height,
        distinct,
        path.display()
    );
}

/// Whether a pixel is within `tolerance` of `color` on every channel.
fn near(pixel: &[u8], color: [u8; 4], tolerance: u8) -> bool {
    pixel.len() == 4
        && pixel
            .iter()
            .zip(color)
            .all(|(actual, expected)| actual.abs_diff(expected) <= tolerance)
}

/// How many distinct RGBA pixels a frame holds.
fn distinct_colors(pixels: &[u8]) -> usize {
    let mut seen: Vec<[u8; 4]> = pixels
        .chunks_exact(4)
        .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
        .collect();
    seen.sort_unstable();
    seen.dedup();
    seen.len()
}

/// A selector for text widgets whose content contains `needle`.
///
/// The register's panes write prose (the machine's model line, the map's
/// legend, the poll line), so a sheet is checked for what it says rather than
/// for a widget that happens to be one whole sentence.
fn says(needle: impl Into<String>) -> impl FnMut(Candidate<'_>) -> Option<()> {
    let needle = needle.into();
    move |candidate| match candidate {
        Candidate::Text { content, .. } if content.contains(needle.as_str()) => Some(()),
        _ => None,
    }
}

/// Whether a frame holds any pixel within `tolerance` of `color`.
fn holds(frame: &iced::window::Screenshot, color: [u8; 4], tolerance: u8) -> bool {
    count_near(frame, color, tolerance) > 0
}

/// How many pixels of a frame are within `tolerance` of `color`.
fn count_near(frame: &iced::window::Screenshot, color: [u8; 4], tolerance: u8) -> usize {
    frame
        .rgba
        .chunks_exact(4)
        .filter(|pixel| near(pixel, color, tolerance))
        .count()
}

#[test]
fn the_index_offers_the_machine_the_map_the_tallies_and_the_keys() {
    let server = common::fleet(LIST, DETAIL);
    let model = machine_from(server.port);
    let mut ui = simulator(view::view(&model));

    for expected in [
        "Machine",
        "Fleet map",
        "plaza-node · x86_64/kvm",
        "2 lane(s) · 3 cell(s)",
        "Keys",
        "register",
        "fleet map",
        "back",
    ] {
        assert!(
            ui.find(says(expected)).is_ok(),
            "the index should show {expected:?}"
        );
    }
    for expected in [
        "2 run(s) · 2 vm(s) up · 2 cid(s) · 1 vsock run(s)",
        "poll 2000 ms · polls 0 · live",
    ] {
        assert!(
            ui.find(says(expected)).is_ok(),
            "the index should show {expected:?}"
        );
    }
}

#[test]
fn the_machine_sheet_lists_every_column_the_daemon_reports() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = machine_from(server.port);
    model.pane = Pane::Machine;

    // 500 s after the daemon started: the uptime the sheet shows.
    assert_eq!(model.machine_uptime_ms(), Some(500_000));
    let mut ui = simulator(view::view(&model));
    for expected in [
        "plaza-node · x86_64/kvm · 24 cpus · 128320 MiB",
        "vsock_mode",
        "uds-muxer",
        "irqchip",
        "ioapic",
        "class",
        "server",
        "cpus",
        "128320 MiB",
        "uptime",
        "8m 20s",
        "started_at",
        "08:53:20 UTC",
        "the transport every VM's vsock is wired through",
    ] {
        assert!(
            ui.find(says(expected)).is_ok(),
            "the machine sheet should show {expected:?}"
        );
    }
}

#[test]
fn the_machine_pane_says_so_when_the_endpoint_does_not_answer() {
    let server = common::always(404, r#"{"error":"no such endpoint"}"#);
    let mut model = Model::at(Endpoint::loopback_at("127.0.0.1", server.port).expect("loopback"));
    model.pane = Pane::Machine;
    let machine = Client::new(model.endpoint.clone()).machine();
    assert!(machine.is_err());
    model.apply(Message::Machine {
        at_ms: model.now_ms,
        result: machine,
    });

    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find(says("the machine endpoint did not answer")).is_ok(),
        "the pane should not invent a machine"
    );
    assert!(ui.find(says("machine: 404: no such endpoint")).is_ok());
}

#[test]
fn the_map_pane_names_what_it_draws() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = machine_from(server.port);
    model.pane = Pane::Map;
    model.selection = Some(Selection {
        run: "r1".to_string(),
        vm: 1,
    });

    let mut ui = simulator(view::view(&model));
    for expected in [
        "The machine is the bounded substrate",
        "2 lane(s) · 3 cell(s)",
        "substrate · plaza-node · x86_64/kvm · 24 cpus · 128320 MiB",
        "selected r1 vm 1",
    ] {
        assert!(ui.find(says(expected)).is_ok(), "the map should say {expected:?}");
    }
}

#[test]
fn the_map_plate_is_drawn_and_the_fleet_table_does_not_draw_it() {
    let server = common::fleet(LIST, DETAIL);
    let mut map_model = machine_from(server.port);
    map_model.pane = Pane::Map;
    map_model.selection = Some(Selection {
        run: "r1".to_string(),
        vm: 1,
    });
    let map_frame = frame_of(map_model.clone());

    assert_eq!((map_frame.size.width, map_frame.size.height), (1280, 860));
    // The substrate region is the machine, drawn as a bounded plate: the whole
    // plate's worth of pixels, not the odd antialiased seam.
    let substrate = count_near(&map_frame, SUBSTRATE, 2);
    assert!(
        substrate > 5_000,
        "the machine's substrate region is not on the map frame ({substrate} pixels)"
    );
    // A killed VM's mark is the red diamond.
    assert!(
        holds(&map_frame, CRIT, 8),
        "no killed VM's mark was drawn on the map"
    );
    // A running VM's mark is the filled green dot. Two of the stub's three
    // VMs are up, so there is more green than one mark's worth.
    let running = count_near(&map_frame, OK, 8);
    assert!(
        running > 300,
        "the running VMs' marks were not drawn ({running} green pixels)"
    );
    // The selected cell carries the register's one hot signal.
    assert!(
        holds(&map_frame, SIGNAL, 8),
        "the selected cell's outline was not drawn"
    );

    // The register pane, drawn from the same model, has no substrate.
    let mut fleet_model = map_model;
    fleet_model.pane = Pane::Fleet;
    let fleet_frame = frame_of(fleet_model);
    let elsewhere = count_near(&fleet_frame, SUBSTRATE, 2);
    assert!(
        elsewhere < substrate / 20,
        "the fleet table should not draw the map's substrate ({elsewhere} pixels)"
    );

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("fleet-map-frame.rgba");
    std::fs::write(&path, map_frame.rgba.as_ref()).expect("the map frame should be written");
    println!(
        "map frame: {substrate} substrate pixels, {} distinct colors, raw RGBA at {}",
        distinct_colors(map_frame.rgba.as_ref()),
        path.display()
    );
}

#[test]
fn the_run_pane_stamps_each_vms_exit_kind_from_its_tail() {
    let server = common::fleet(LIST, EXITS);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Run("r1".to_string())));
    let run = Client::new(model.endpoint.clone())
        .run("r1")
        .expect("the stub answers the run");
    model.apply(Message::Run {
        at_ms: model.now_ms,
        id: "r1".to_string(),
        result: Ok(run),
    });

    let mut ui = simulator(view::view(&model));
    for expected in [
        "SLEPT",
        "RESET",
        "reboot: Power down",
        "machine restart",
        "cid 40",
        "cid 41",
    ] {
        assert!(
            ui.find(says(expected)).is_ok(),
            "the run pane should show {expected:?}"
        );
    }
    for expected in ["vm 00", "vm 01"] {
        assert!(
            ui.find(expected).is_ok(),
            "the run pane should list {expected:?}"
        );
    }
}

#[test]
fn a_vm_row_aims_the_console_and_the_transcript_follows_it() {
    let server = common::fleet(LIST, EXITS);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Run("r1".to_string())));
    let run = Client::new(model.endpoint.clone())
        .run("r1")
        .expect("the stub answers the run");
    model.apply(Message::Run {
        at_ms: model.now_ms,
        id: "r1".to_string(),
        result: Ok(run),
    });

    // The first VM's capture is what the transcript shows.
    {
        let mut ui = simulator(view::view(&model));
        assert!(ui.find(says("reboot: Power down")).is_ok());
    }

    // Pressing the second VM's row selects it, and the pane follows.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("vm 01").expect("the VM row is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        let effect = model.apply(message);
        assert!(effect.is_idle(), "selecting a VM needs no daemon call");
    }
    assert_eq!(model.target_vm("r1"), 1);
    assert_eq!(model.selection.as_ref().expect("a selection").vm, 1);

    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find(says("machine restart")).is_ok(),
        "the transcript now shows the selected VM's tail"
    );
    assert!(
        ui.find(says("vm 1 · cid 41")).is_ok(),
        "the console names its target"
    );
}

#[test]
fn the_console_offers_the_guests_commands_and_sends_a_line() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Run("r1".to_string())));
    let run = Client::new(model.endpoint.clone())
        .run("r1")
        .expect("the stub answers the run");
    model.apply(Message::Run {
        at_ms: model.now_ms,
        id: "r1".to_string(),
        result: Ok(run),
    });
    model.apply(Message::ConsoleEdit("uname".to_string()));
    let client = Client::new(model.endpoint.clone());

    let mut ui = simulator(view::view(&model));
    for expected in [
        "Write to the serial console",
        "vm 0 · cid 50",
        // The guest's own command set, as the quick buttons offer it.
        "help",
        "uname",
        "cpu",
        "mem",
        "uptime",
        "poweroff",
    ] {
        assert!(ui.find(says(expected)).is_ok(), "the console should show {expected:?}");
    }

    ui.click("SEND").expect("SEND is clickable");
    let messages: Vec<Message> = ui.into_messages().collect();
    assert!(
        messages.contains(&Message::ConsoleSend),
        "pressing SEND asks the model to send, got {messages:?}"
    );
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }
    assert_eq!(model.health(), agentplaza_desktop::model::Health::Live);
    assert_eq!(model.console.history, vec!["uname".to_string()]);
    assert_eq!(model.console.line, "");

    // What the console put on the wire, read off the stub.
    let requests = server.requests();
    let written = requests
        .iter()
        .find(|request| request.method == "POST")
        .expect("the console wrote to the daemon");
    assert_eq!(written.target, "/api/v0/runs/r1/input");
    assert_eq!(written.body, r#"{"data":"uname\n","vm":0}"#);
}

#[test]
fn the_history_arrows_offer_the_lines_already_sent() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Run("r1".to_string())));
    let run = Client::new(model.endpoint.clone())
        .run("r1")
        .expect("the stub answers the run");
    model.apply(Message::Run {
        at_ms: model.now_ms,
        id: "r1".to_string(),
        result: Ok(run),
    });
    let client = Client::new(model.endpoint.clone());

    for line in ["help", "uname"] {
        model.apply(Message::ConsoleEdit(line.to_string()));
        let effect = model.apply(Message::ConsoleSend);
        perform(effect, &mut model, &client);
    }
    assert_eq!(model.console.history, vec!["help".to_string(), "uname".to_string()]);

    // The arrows are the console's: pressing ↑ puts the last line in the box.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("↑").expect("the history arrow is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        model.apply(message);
    }
    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find("uname").is_ok(),
        "the walked line is in the input box"
    );
}

#[test]
fn a_quick_command_button_sends_that_command() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Run("r1".to_string())));
    let run = Client::new(model.endpoint.clone())
        .run("r1")
        .expect("the stub answers the run");
    model.apply(Message::Run {
        at_ms: model.now_ms,
        id: "r1".to_string(),
        result: Ok(run),
    });
    let client = Client::new(model.endpoint.clone());

    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("poweroff").expect("the quick button is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }

    let requests = server.requests();
    let written = requests
        .iter()
        .find(|request| request.method == "POST")
        .expect("the quick button wrote to the daemon");
    assert_eq!(written.target, "/api/v0/runs/r1/input");
    assert_eq!(written.body, r#"{"data":"poweroff\n","vm":0}"#);
}

#[test]
fn the_machine_key_reaches_the_machine_screen_through_the_register() {
    // The keys the register binds, applied the way the runtime applies them,
    // plus the machine endpoint read by the real client.
    let server = common::fleet(LIST, DETAIL);
    let mut model = machine_from(server.port);
    let client = Client::new(model.endpoint.clone());
    let effect = model.apply(Message::Key(agentplaza_desktop::model::KeyAction::Machine));
    assert!(effect.poll_machine);
    perform(effect, &mut model, &client);

    let machine = model.machine.as_ref().expect("the machine arrived");
    assert_eq!(machine.name, "plaza-node");
    assert_eq!(machine.vsock_mode, "uds-muxer");

    let port = server.port;
    assert!(
        server
            .requests()
            .iter()
            .any(|request| request.method == "GET" && request.target == "/api/v0/machine"),
        "the machine was read over a real socket"
    );

    // And the whole application — its own subscriptions, its own boot tick —
    // still draws the register's frame.
    let frame = iced_test::screenshot(
        &app::application(config(port)),
        &theme::theme(),
        Size::new(1280.0, 860.0),
        1.0,
        Duration::from_millis(1_200),
    );
    assert_eq!((frame.size.width, frame.size.height), (1280, 860));
    assert!(
        holds(&frame, PAPER, 2) && holds(&frame, INK, 8),
        "the running register draws its page and its ink"
    );
}

#[test]
fn the_stubbed_machine_body_is_the_shape_the_daemon_sends() {
    // The fixture the render tests use is the daemon's own body, parsed by the
    // client's own type: a fixture that drifted would fail here first.
    let machine: agentplaza_desktop::api::Machine =
        serde_json::from_str(MACHINE).expect("the machine fixture parses");
    assert_eq!(machine.name, "plaza-node");
    assert_eq!(machine.class, "server");
    assert_eq!(machine.memory_mib, 128_320);
}

#[test]
fn the_spawn_form_offers_the_presets_with_their_footprints() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Spawn));

    let mut ui = simulator(view::view(&model));
    for expected in [
        "2 VMs + vsock",
        "8 VMs",
        "50-VM scale",
        "Quick boots",
        "2 × 512 MiB ≈ 1.0 GiB guest RAM",
        "8 × 512 MiB ≈ 4.0 GiB guest RAM",
        "50 × 128 MiB ≈ 6.3 GiB guest RAM",
        "two VMs on cid 10, a full session each",
    ] {
        assert!(
            ui.find(says(expected)).is_ok(),
            "the form should show {expected:?}"
        );
    }
}

#[test]
fn a_preset_click_fills_the_form_and_boot_run_sends_exactly_that_body() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Spawn));
    let client = Client::new(model.endpoint.clone());

    // The click the operator makes: the scale preset, nothing else.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("50-VM scale").expect("the preset is clickable");
        ui.into_messages().collect()
    };
    assert!(
        messages.contains(&Message::Preset(agentplaza_desktop::form::Preset::Scale)),
        "the preset button asks for its own numbers, got {messages:?}"
    );
    for message in messages {
        let effect = model.apply(message);
        assert!(effect.is_idle(), "a preset fills the form and sends nothing");
    }
    // The form now says exactly what the preset means, and every box is filled
    // with something the operator can still change.
    assert_eq!(model.form.field(agentplaza_desktop::form::Field::N), "50");
    assert_eq!(model.form.field(agentplaza_desktop::form::Field::Vcpus), "1");
    assert_eq!(
        model.form.field(agentplaza_desktop::form::Field::MemoryMib),
        "128"
    );
    assert_eq!(model.form.field(agentplaza_desktop::form::Field::Beats), "");
    assert_eq!(
        model.form.field(agentplaza_desktop::form::Field::VsockCid),
        ""
    );

    // The form the operator submits: what went on the wire, byte for byte.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("BOOT RUN").expect("the boot button is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }

    let requests = server.requests();
    let created = requests
        .iter()
        .find(|request| request.method == "POST" && request.target == "/api/v0/runs")
        .expect("the preset's form was sent");
    assert_eq!(
        created.body,
        r#"{"n":50,"vcpus":1,"memory_mib":128,"beats":null,"vsock_cid":null}"#
    );
    assert_eq!(
        model.notice.as_ref().expect("a notice").text,
        "run r1 created",
        "the stub answered the create, and the register says so"
    );
}

#[test]
fn the_export_button_writes_the_selected_vms_transcript_and_reads_back_the_same_bytes() {
    let server = common::fleet(LIST, EXITS);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Run("r1".to_string())));
    let run = Client::new(model.endpoint.clone())
        .run("r1")
        .expect("the stub answers the run");
    // A fixed clock, so the transcript's header is a known string.
    model.apply(Message::Run {
        at_ms: 1_760_000_500_000,
        id: "r1".to_string(),
        result: Ok(run),
    });
    let client = Client::new(model.endpoint.clone());

    let path = model.export.path.clone();
    assert!(
        path.ends_with("plaza-r1-0.txt"),
        "the box is pre-filled for the selected vm: {path}"
    );
    let _ = std::fs::remove_file(&path);

    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        // What the row says it will write, before anything is written.
        let bytes = model.detail.as_ref().expect("the run").vms[0]
            .serial
            .as_deref()
            .expect("the first VM's capture")
            .len();
        assert_eq!(bytes, 42, "the fixture's first capture");
        assert!(
            ui.find(says(&format!("vm 0 · {bytes} byte(s) of capture →")))
                .is_ok(),
            "the row names the file"
        );
        assert!(ui.find(says("Export the transcript")).is_ok());
        assert!(ui.find(says("this vm")).is_ok());
        assert!(ui.find(says("all vms")).is_ok());
        ui.click("EXPORT").expect("EXPORT is clickable");
        ui.into_messages().collect()
    };
    assert!(
        messages.contains(&Message::Export),
        "pressing EXPORT asks the model to export, got {messages:?}"
    );
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }

    // The bytes on disk are the transcript the pane holds, header and all.
    let written = std::fs::read_to_string(&path).expect("the transcript was written");
    let detail = model.detail.clone().expect("the run");
    assert_eq!(
        written,
        agentplaza_desktop::export::vm_transcript(&detail, &detail.vms[0], 1_760_000_500_000)
    );
    assert!(written.starts_with("# AgentPlaza transcript\n# run r1 · vm 0 · exited"));
    assert!(written.ends_with("reboot: Power down\r\n"), "{written:?}");
    assert_eq!(
        model.notice.as_ref().expect("a notice").text,
        format!("exported {} byte(s) of run r1 vm 0 to {path}", written.len())
    );

    // Every VM, labelled, to the run's own file.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("all vms").expect("the target switch is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }
    let run_path = model.export.path.clone();
    assert!(run_path.ends_with("plaza-r1-all.txt"), "{run_path}");

    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("EXPORT").expect("EXPORT is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }
    let all = std::fs::read_to_string(&run_path).expect("the run's transcript was written");
    assert_eq!(
        all,
        agentplaza_desktop::export::run_transcript(&detail, 1_760_000_500_000)
    );
    assert!(all.contains("# vm 1 · killed · cid 41 · exit reset\n"), "{all:?}");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&run_path);
}

#[test]
fn the_report_target_writes_the_runs_report_and_reads_its_sessions_back() {
    let server = common::fleet(LIST, REPORT_DETAIL);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Run("r1".to_string())));
    let run = Client::new(model.endpoint.clone())
        .run("r1")
        .expect("the stub answers the run");
    model.apply(Message::Run {
        at_ms: 1_760_000_500_000,
        id: "r1".to_string(),
        result: Ok(run),
    });
    let client = Client::new(model.endpoint.clone());

    // The switch offers the report beside the two transcript targets, and
    // pressing it moves the box to the report's own default path.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        assert!(ui.find(says("report")).is_ok(), "the report target is offered");
        ui.click("report").expect("the report target is clickable");
        ui.into_messages().collect()
    };
    assert!(
        messages.contains(&Message::ExportTarget(ExportTarget::Report)),
        "pressing it asks for the report target, got {messages:?}"
    );
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }
    let path = model.export.path.clone();
    assert!(path.ends_with("plaza-r1-report.txt"), "{path}");
    let _ = std::fs::remove_file(&path);

    // The row names what lands where, and EXPORT writes it.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        assert!(
            ui.find(says("the report of r1")).is_ok(),
            "the row names the report it will write"
        );
        ui.click("EXPORT").expect("EXPORT is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }

    let written = std::fs::read_to_string(&path).expect("the report was written");
    let detail = model.detail.clone().expect("the run");
    assert_eq!(
        written,
        agentplaza_desktop::export::run_report(&detail, 1_760_000_500_000),
        "the bytes on disk are the pane's own report"
    );
    // The sessions the captures show are in the file, derived rather than
    // invented: vm 0 completed the frozen session, vm 1 has not said bye.
    assert!(
        written.contains("session: hello→welcome→echo✓→uptime✓→bye, 2 tool calls\n"),
        "{written}"
    );
    assert!(
        written.contains("session: keepalive, 1 heartbeat, 1 tool call, no bye seen\n"),
        "{written}"
    );
    assert!(
        written.contains("# sessions: 1 complete · 1 keepalive · 0 none seen\n"),
        "{written}"
    );
    assert_eq!(
        model.notice.as_ref().expect("a notice").text,
        format!("exported {} byte(s) of the report of run r1 to {path}", written.len())
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_export_that_cannot_land_is_a_refusal_on_the_sheet_not_a_silent_nothing() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    model.apply(Message::Show(Pane::Run("r1".to_string())));
    let run = Client::new(model.endpoint.clone())
        .run("r1")
        .expect("the stub answers the run");
    model.apply(Message::Run {
        at_ms: model.now_ms,
        id: "r1".to_string(),
        result: Ok(run),
    });
    let client = Client::new(model.endpoint.clone());

    // A path under a directory that does not exist.
    let path = format!(
        "/nonexistent-plaza-dir-{}/plaza-r1-0.txt",
        std::process::id()
    );
    model.apply(Message::ExportEdited(path.clone()));
    assert_eq!(model.export.to_write(), path);

    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("EXPORT").expect("EXPORT is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }

    let notice = model.notice.as_ref().expect("a notice");
    assert_eq!(notice.kind, NoticeKind::Failed);
    assert!(
        notice.text.starts_with(&format!("export {path}: ")),
        "the refusal names the path it could not write: {}",
        notice.text
    );
    assert!(!std::path::Path::new(&path).exists());
    // The registry is still live: the disk failed, not the daemon.
    assert_eq!(model.health(), agentplaza_desktop::model::Health::Live);

    let mut ui = simulator(view::view(&model));
    assert!(ui.find("REFUSED").is_ok(), "the sheet shows the refusal");
}

#[test]
fn the_help_key_lays_the_sheet_over_the_frame_the_register_was_drawing() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = machine_from(server.port);
    model.pane = Pane::Map;
    model.selection = Some(Selection {
        run: "r1".to_string(),
        vm: 1,
    });

    // The key the runtime routes, then the sheet it puts up.
    let effect = model.apply(Message::Key(agentplaza_desktop::model::KeyAction::Help));
    assert!(effect.is_idle());
    assert!(model.help);

    let mut ui = simulator(view::view(&model));
    for expected in [
        "keys",
        "this help",
        "back; closes this sheet",
        "console history",
        "CLOSE",
        "the register keeps polling behind this sheet",
        "are buttons on the run pane, not keys",
    ] {
        assert!(
            ui.find(says(expected)).is_ok(),
            "the help sheet should show {expected:?}"
        );
    }

    // The close button is the same switch.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("CLOSE").expect("CLOSE is clickable");
        ui.into_messages().collect()
    };
    assert!(
        messages.contains(&Message::Help(false)),
        "CLOSE takes the sheet down, got {messages:?}"
    );

    // And the frame: the sheet drawn over a register that is still there.
    let mut with_help = model.clone();
    with_help.help = true;
    let mut plain = model.clone();
    plain.help = false;
    let covered = frame_of(with_help);
    let uncovered = frame_of(plain);

    assert_eq!((covered.size.width, covered.size.height), (1280, 860));
    // The help box is the sheet's own colour: a solid run of it, centred.
    let boxed = count_near(&covered, SHEET, 2);
    assert!(
        boxed > 100_000,
        "the help sheet was not drawn ({boxed} sheet pixels)"
    );
    // The register behind is still drawn: ink, wherever the index's text was,
    // survives a scrim that blends ink over ink.
    assert!(
        holds(&covered, INK, 2),
        "the frame under the sheet is gone rather than covered"
    );
    // The page is covered: its own colour is essentially absent, and what
    // covers it is one blend that the plain frame does not hold at all.
    let page_covered = count_near(&covered, PAPER, 2);
    let page_plain = count_near(&uncovered, PAPER, 2);
    assert!(page_plain > 100_000, "the plain frame is a page ({page_plain})");
    assert!(
        page_covered < page_plain / 50,
        "the page is not dimmed under the sheet ({page_covered} of {page_plain} pixels)"
    );
    let (dim, dim_pixels) = dominant(&covered);
    assert!(
        dim_pixels > 300_000,
        "a scrim should cover most of the window ({dim_pixels} pixels)"
    );
    // That blend is the window's own colour: it covers a hundred times more of
    // the frame under the sheet than the antialiased edges it happens to
    // coincide with elsewhere.
    let dim_elsewhere = count_near(&uncovered, dim, 2);
    assert!(
        dim_pixels > 100 * dim_elsewhere.max(1),
        "the dimmed colour {dim:?} exists only under the sheet ({dim_pixels} vs {dim_elsewhere})"
    );

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("help-frame.rgba");
    std::fs::write(&path, covered.rgba.as_ref()).expect("the help frame should be written");
    println!(
        "help frame: {boxed} sheet pixels over {dim_pixels} scrim pixels, raw RGBA at {}",
        path.display()
    );
}

#[test]
fn a_lane_wider_than_the_plate_draws_what_fits_and_says_what_it_could_not() {
    // The scale preset's fleet, without a daemon: one run of fifty VMs.
    let mut model = Model::new();
    model.machine = Some(agentplaza_desktop::api::Machine {
        name: "plaza-node".to_string(),
        arch: "x86_64".to_string(),
        backend: "kvm".to_string(),
        irqchip: "ioapic".to_string(),
        class: "server".to_string(),
        cpus: 24,
        memory_mib: 128_320,
        vsock_mode: "uds-muxer".to_string(),
        started_at_ms: 1_760_000_000_000,
    });
    model.runs = vec![agentplaza_desktop::api::Run {
        id: "r1".to_string(),
        state: agentplaza_desktop::api::RunState::Running,
        vcpus: 1,
        memory_mib: 128,
        beats: None,
        created_at: 1_760_000_000_000,
        ended_at: None,
        vms: (0..50)
            .map(|id| agentplaza_desktop::api::Vm {
                id,
                guest_cid: None,
                role: None,
                state: agentplaza_desktop::api::VmState::Running,
                serial: None,
            })
            .collect(),
    }];
    model.pane = Pane::Map;

    {
        let mut ui = simulator(view::view(&model));
        assert!(
            ui.find(says("1 lane(s) · 50 cell(s)")).is_ok(),
            "the counts stay real"
        );
    }

    let frame = frame_of(model);
    // Fifteen of the fifty marks fit; the rest is past the plate's edge.
    let running = count_near(&frame, OK, 8) as f32;
    let one_mark = std::f32::consts::PI * 7.0 * 7.0; // π r² for the 7 px mark
    assert!(
        running > one_mark * 12.0 && running < one_mark * 18.0,
        "the plate should draw the fifteen marks it can hold, not fifty or one: {running} green pixels"
    );
    // And it says so, in the caution's amber. Nine-pixel canvas text
    // antialiases into the substrate, so this is the hue of the amber ink
    // rather than one exact colour, and it excludes the light taint a selected
    // register row is pressed in.
    let note = frame
        .rgba
        .chunks_exact(4)
        .filter(|pixel| amber(pixel))
        .count();
    assert!(
        note > 100,
        "the clipped lane drew no note ({note} amber pixels)"
    );

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("clipped-lane-frame.rgba");
    std::fs::write(&path, frame.rgba.as_ref()).expect("the frame should be written");
    println!(
        "clipped lane frame: {running} mark pixels, {note} note pixels at {}",
        path.display()
    );
}

/// Whether a pixel is the caution's amber ink: red over green over blue, clear
/// of the greys, of the map's red and green, and of the light taint a selected
/// register row is pressed in.
fn amber(pixel: &[u8]) -> bool {
    pixel.len() == 4
        && pixel[0] > pixel[1]
        && pixel[1] > pixel[2]
        && pixel[0] - pixel[2] > 0x30
        && pixel[0] - pixel[2] <= 0x95
        && pixel[1] - pixel[2] > 0x18
        && pixel[2] < 0xb0
}

/// The stub's list body for a run of `live` VMs of `memory_mib` each: the
/// scale preset's shape, which is the session chart's own story.
fn scale_body(memory_mib: u32, live: u32) -> String {
    let vms: Vec<String> = (0..live)
        .map(|id| {
            format!(
                "{{\"id\":{id},\"guest_cid\":{},\"state\":\"running\"}}",
                10 + id
            )
        })
        .collect();
    format!(
        "{{\"runs\":[{{\"id\":\"r9\",\"state\":\"running\",\"vcpus\":1,\"memory_mib\":{memory_mib},\
         \"beats\":12,\"created_at\":1760000000000,\"ended_at\":null,\"vms\":[{}]}}]}}",
        vms.join(",")
    )
}

/// A model whose session is the sequence of list answers given, each read off
/// a real socket by the real client and applied the way the runtime applies
/// it: the observations the charts draw are the ones a poll would produce.
fn observed(bodies: &[String]) -> Model {
    let mut queue: Vec<(u16, String)> = vec![(200, MACHINE.to_string())];
    for body in bodies {
        queue.push((200, body.clone()));
    }
    let stub = common::replies(queue);
    let mut model = Model::at(Endpoint::loopback_at("127.0.0.1", stub.port).expect("loopback"));
    let client = Client::new(model.endpoint.clone());

    let machine = client.machine().expect("the stub answers the machine");
    model.apply(Message::Machine {
        at_ms: 1_760_000_499_000,
        result: Ok(machine),
    });
    for (step, _) in bodies.iter().enumerate() {
        let page = client.runs().expect("the stub answers the list");
        model.apply(Message::Runs {
            at_ms: 1_760_000_500_000 + step as u64 * 2_000,
            result: Ok(page),
        });
    }
    model
}

/// The scale preset's ramp, as the session saw it: fifty 128 MiB guests come
/// up, hold, drain to eleven behind their heartbeat deadline, and go.
fn scale_ramp() -> Vec<String> {
    [(0u32), 50, 50, 50, 11, 0]
        .into_iter()
        .map(|live| scale_body(128, live))
        .collect()
}

#[test]
fn the_telemetry_pane_says_so_before_it_has_a_figure() {
    let server = common::fleet(LIST, DETAIL);
    // The machine alone: no list poll yet, so nothing has been observed.
    let mut model = machine_only(server.port);
    model.apply(Message::Show(Pane::Telemetry));
    assert_eq!(model.pane, Pane::Telemetry);
    assert!(model.history.is_empty());

    // No observation at all: every chart says so instead of drawing junk.
    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find(says("only 0 observation(s) so far")).is_ok(),
        "an empty session says so"
    );
    assert!(ui.find(says("Telemetry")).is_ok());
    assert!(ui.find(says("Export the session (CSV)")).is_ok());
    assert!(ui.find(says("the whole fleet")).is_ok());
    assert!(ui.find(says("committed guest RAM · over 0s")).is_ok());
    assert!(ui.find(says("fleet serial throughput")).is_ok());

    // Two observations: still a start, not a figure.
    let mut model = observed(&scale_ramp()[..2]);
    model.apply(Message::Show(Pane::Telemetry));
    assert_eq!(model.history.len(), 2);
    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find(says("only 2 observation(s) so far")).is_ok(),
        "two observations are still a start"
    );
    assert!(ui.find(says("6.3 GiB")).is_ok(), "the commitment is named");
    assert!(ui.find(says("2 kept of 2")).is_ok());

    // Three: the figure lands, and the caption says what it covers.
    let mut model = observed(&scale_ramp()[..3]);
    model.apply(Message::Show(Pane::Telemetry));
    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find(says("only 3 observation(s) so far")).is_err(),
        "three observations are a figure"
    );
    assert!(
        ui.find(says("committed guest RAM · over 4s · now 6.3 GiB · peak 6.3 GiB"))
            .is_ok(),
        "the caption names the figure and its window"
    );
    assert!(ui.find(says("every poll consecutive")).is_ok());
}

#[test]
fn the_telemetry_plate_is_drawn_and_a_picked_runs_band_is_the_signal() {
    let mut model = observed(&scale_ramp());
    model.apply(Message::Show(Pane::Telemetry));
    assert_eq!(model.history.len(), 6);
    assert_eq!(
        model.history.latest().expect("an observation").ram_mib,
        0,
        "the drain is the last thing the session saw"
    );
    assert_eq!(model.fleet_lps(), 0.0, "nothing runs, so nothing is counted");

    // With no pick, the fleet's own series is ink and nothing carries the
    // register's one hot signal.
    let plain = frame_of(model.clone());
    assert_eq!((plain.size.width, plain.size.height), (1280, 860));
    assert!(
        count_near(&plain, INK, 6) > 5_000,
        "the charts draw their series in the register's ink"
    );
    assert_eq!(
        count_near(&plain, SIGNAL, 6),
        0,
        "nothing is picked, so nothing is drawn in the signal"
    );

    // Picked, r9's own band is the signal stroke.
    let mut picked = model.clone();
    picked.telemetry_run = Some("r9".to_string());
    assert_eq!(picked.telemetry_pick(), Some("r9"));
    let with_pick = frame_of(picked);
    let signal = count_near(&with_pick, SIGNAL, 8);
    assert!(
        signal > 1_000,
        "the picked run's band was not drawn in the signal ({signal} pixels)"
    );

    // A fleet that has been asked for more guest memory than the host has is
    // where the host's own total becomes a ruled line inside the chart.
    let mut over = observed(&[scale_body(200_000, 1), scale_body(200_000, 1), scale_body(200_000, 1)]);
    over.apply(Message::Show(Pane::Telemetry));
    assert_eq!(
        over.machine.as_ref().expect("the machine").memory_mib,
        128_320
    );
    let over_frame = frame_of(over);
    assert!(
        count_near(&over_frame, WARN, 8) > 500,
        "the host's total is not ruled on an overcommitted chart ({} warn pixels)",
        count_near(&over_frame, WARN, 8)
    );

    for (name, frame) in [
        ("telemetry-frame", &plain),
        ("telemetry-picked-frame", &with_pick),
    ] {
        let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.rgba"));
        std::fs::write(&path, frame.rgba.as_ref()).expect("the frame should be written");
        println!(
            "{name}: {} signal pixels, {} distinct colours, raw RGBA at {}",
            count_near(frame, SIGNAL, 8),
            distinct_colors(frame.rgba.as_ref()),
            path.display()
        );
    }
}

#[test]
fn the_csv_button_writes_the_sessions_ramp_and_reads_the_bytes_back() {
    let mut model = observed(&scale_ramp());
    model.apply(Message::Show(Pane::Telemetry));
    let client = Client::new(model.endpoint.clone());

    let path = model.csv.path.clone();
    assert!(path.ends_with(".csv"), "the box is pre-filled: {path}");
    let _ = std::fs::remove_file(&path);

    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        assert!(ui.find(says("6 fleet row(s) + 6 run row(s)")).is_ok());
        assert!(ui.find(says("observed_at, elapsed_s, scope, vms_live,")).is_ok());
        ui.click("EXPORT CSV").expect("EXPORT CSV is clickable");
        ui.into_messages().collect()
    };
    assert!(
        messages.contains(&Message::CsvExport),
        "pressing EXPORT CSV asks the model to export, got {messages:?}"
    );
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }

    // The bytes on disk are exactly the session's own observations, and the
    // ramp reads out of them: none, the 6.3 GiB plateau, the drain, none.
    let written = std::fs::read_to_string(&path).expect("the session was written");
    assert_eq!(
        written,
        agentplaza_desktop::history::telemetry_csv(&model.history)
    );
    let rows: Vec<&str> = written.lines().skip(1).collect();
    assert_eq!(rows.len(), 12, "six observations, fleet and r9 each");
    assert_eq!(rows[0], "1760000500000,0.000,fleet,0,0,0.00");
    assert_eq!(rows[1], "1760000500000,0.000,r9,0,0,0.00");
    assert_eq!(rows[2], "1760000502000,2.000,fleet,50,6400,0.00");
    assert_eq!(rows[6], "1760000506000,6.000,fleet,50,6400,0.00");
    assert_eq!(rows[7], "1760000506000,6.000,r9,50,6400,0.00");
    assert_eq!(rows[8], "1760000508000,8.000,fleet,11,1408,0.00");
    assert_eq!(rows[10], "1760000510000,10.000,fleet,0,0,0.00");
    assert_eq!(rows[11], "1760000510000,10.000,r9,0,0,0.00");
    assert_eq!(
        model.notice.as_ref().expect("a notice").kind,
        NoticeKind::Done
    );
    assert_eq!(
        model.notice.as_ref().expect("a notice").text,
        format!("exported {} byte(s) of the session to {path}", written.len())
    );

    // The export is a pure function of the window: the same observations write
    // the same bytes again.
    let messages: Vec<Message> = {
        let mut ui = simulator(view::view(&model));
        ui.click("EXPORT CSV").expect("EXPORT CSV is clickable");
        ui.into_messages().collect()
    };
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }
    assert_eq!(
        std::fs::read_to_string(&path).expect("the session was written"),
        written
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_run_pane_charts_the_selected_vms_rate_and_keeps_the_numeric_column() {
    let server = common::fleet(LIST, DETAIL);
    let mut model = machine_from(server.port);
    let client = Client::new(model.endpoint.clone());
    perform(
        model.apply(Message::Show(Pane::Run("r1".to_string()))),
        &mut model,
        &client,
    );
    assert_eq!(model.pane, Pane::Run("r1".to_string()));

    // Three serial observations of the run the pane is following, so the
    // selected VM's ring has a shape.
    for (step, lines) in [(0u32, 3usize), (1, 40), (2, 90)] {
        let capture: String = (0..lines).map(|i| format!("line {i}\n")).collect();
        let mut run = model.detail.clone().expect("the run");
        run.vms.truncate(1);
        run.vms[0].serial = Some(capture);
        model.apply(Message::Run {
            at_ms: 1_760_000_500_000 + u64::from(step) * 2_000,
            id: "r1".to_string(),
            result: Ok(run),
        });
    }
    // One observation from opening the pane, then three more: the ring holds
    // what the run's own fetches measured.
    let series = model.history.vm_series("r1", 0);
    assert_eq!(series.len(), 4);
    assert_eq!(series[0], 0.0, "the first sighting is a baseline");
    assert!(
        series[3] > series[2],
        "the burst is still climbing: {series:?}"
    );

    let mut ui = simulator(view::view(&model));
    assert!(ui.find(says("Serial rate · vm 0 (lines/s)")).is_ok());
    assert!(ui.find(says("4 point(s) of 120")).is_ok());
    // The VM row keeps its own number; the chart is the shape beside it.
    assert!(ui.find(says("ln/s")).is_ok(), "the rate column stays");

    let frame = frame_of(model.clone());
    assert!(
        holds(&frame, INK, 6),
        "the sparkline draws the vm's series in the register's ink"
    );

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("sparkline-frame.rgba");
    std::fs::write(&path, frame.rgba.as_ref()).expect("the frame should be written");
    println!(
        "run pane with a sparkline: {} distinct colours, raw RGBA at {}",
        distinct_colors(frame.rgba.as_ref()),
        path.display()
    );
}

/// A JSON string literal for `text`, so the stream's frames below hold the
/// daemon's own escaping rather than a hand-written one.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The capture `DETAIL`'s first VM holds: what a poll reads whole.
const STREAM_CAPTURE: &str = "login: plaza\r\n$ uname -a\r\nLinux plaza 7.2.8 x86_64\r\n";

/// The bytes the guest writes after that capture: the stream's own, which the
/// register must show without waiting for a poll.
const STREAM_TAIL: &str = "$ halted\n";

/// The daemon's `GET /api/v0/events` frames for this test: the run ends, and
/// the VM's serial arrives whole — which is also exactly what a reconnecting
/// console sees replayed, so the merge has to be idempotent about it.
fn stream_frames() -> String {
    format!(
        "event: run\ndata: {{\"id\":\"r1\",\"state\":\"done\"}}\n\n\
         : heartbeat\n\n\
         event: serial\ndata: {{\"run\":\"r1\",\"vm\":0,\"offset\":0,\"text\":{}}}\n\n",
        quoted(&format!("{STREAM_CAPTURE}{STREAM_TAIL}"))
    )
}

/// The config a test asks for: the daemon to read and the cadence to poll at.
fn config_at(port: u16, refresh_ms: u64) -> Config {
    Config {
        endpoint: Endpoint::loopback_at("127.0.0.1", port).expect("loopback"),
        timeout: Duration::from_secs(5),
        refresh_ms,
    }
}

#[test]
fn the_stream_settles_the_register_and_the_transcript_between_polls() {
    // The list endpoint never says r1 ended: only the event stream does, and
    // the cadence is the 30 s ceiling, so no second poll can run while this
    // test does. Whatever moves the register and the transcript, it is the
    // stream.
    let frames = stream_frames();
    let server = common::live(LIST, DETAIL, &frames);
    let endpoint = Endpoint::loopback_at("127.0.0.1", server.port).expect("loopback");

    // The capture a poll reads whole, straight off the stub, so the assertion
    // below is about the merge and not about a fixture copied by hand.
    let polled = Client::new(endpoint.clone())
        .run("r1")
        .expect("the stub answers the run")
        .serial()
        .to_string();
    assert!(
        format!("{STREAM_CAPTURE}{STREAM_TAIL}").starts_with(&polled),
        "the frames must open with exactly the capture the poll reads"
    );

    let program = app::application(config_at(server.port, 30_000));
    let (sender, mut receiver) = iced::futures::channel::mpsc::channel(64);
    let mut emulator = iced_test::emulator::Emulator::new(
        sender,
        &program,
        iced_test::emulator::Mode::Immediate,
        Size::new(1280.0, 860.0),
    );

    // The application's own subscriptions, its own stream thread and its own
    // client: nothing here applies an event by hand.
    let mut pump = |emulator: &mut iced_test::emulator::Emulator<_>, millis: u64| {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_millis(millis) {
            if let Ok(iced_test::emulator::Event::Action(action)) = receiver.try_recv() {
                emulator.perform(&program, action);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    pump(&mut emulator, 1_200);

    // Open the run: the pane's own fetch is what syncs a capture the stream
    // can then extend.
    emulator.update(&program, Message::Show(Pane::Run("r1".to_string())));
    pump(&mut emulator, 1_500);

    let frame = emulator.screenshot(&program, &theme::theme(), 1.0);
    assert_eq!((frame.size.width, frame.size.height), (1280, 860));

    let (model, _) = emulator.into_state();
    assert_eq!(model.polls, 1, "only the boot tick polled: nothing else asked");
    assert_eq!(
        model.runs[0].state,
        agentplaza_desktop::api::RunState::Done,
        "the run's state came off the stream, not off a poll"
    );
    assert_eq!(model.health(), agentplaza_desktop::model::Health::Live);
    assert_eq!(model.pane, Pane::Run("r1".to_string()));

    let capture = model
        .detail
        .as_ref()
        .expect("the open run")
        .serial()
        .to_string();
    assert_eq!(
        capture,
        format!("{polled}{STREAM_TAIL}"),
        "the transcript is the polled capture plus the streamed bytes, once"
    );
    assert_eq!(
        model.serial.length("r1", 0),
        capture.len() as u64,
        "the feed's own bookkeeping agrees with the bytes it merged"
    );
    assert_eq!(
        model.serial.lines("r1", 0),
        Some(capture.bytes().filter(|byte| *byte == b'\n').count() as u64)
    );

    // And the sheet draws it: the ledger heading names the span, and the
    // stream's own line is a row of it.
    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find(says(&format!(
            "Serial · vm 0 · {} line(s) · byte 0–{}",
            capture.lines().count(),
            capture.len()
        )))
        .is_ok(),
        "the transcript names the bytes it drew"
    );
    assert!(ui.find(says("$ halted")).is_ok(), "the streamed line is drawn");
    assert!(ui.find(says("DONE")).is_ok(), "the run's stamp moved too");

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("stream-frame.rgba");
    std::fs::write(&path, frame.rgba.as_ref()).expect("the frame should be written");
    println!(
        "the stream's register frame: {} distinct colours, raw RGBA at {}",
        distinct_colors(frame.rgba.as_ref()),
        path.display()
    );
    println!(
        "stream run: polls {}, r1 {:?}, transcript {} bytes ({} polled + {} streamed)",
        model.polls,
        model.runs[0].state,
        capture.len(),
        polled.len(),
        STREAM_TAIL.len()
    );
}

/// The colour covering the most pixels of a frame, and how many it covers.
fn dominant(frame: &iced::window::Screenshot) -> ([u8; 4], usize) {
    use std::collections::HashMap;
    let mut counts: HashMap<[u8; 4], usize> = HashMap::new();
    for pixel in frame.rgba.chunks_exact(4) {
        *counts
            .entry([pixel[0], pixel[1], pixel[2], pixel[3]])
            .or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .expect("a frame has pixels")
}

// ---------------------------------------------------------------- the role fleet

#[test]
fn the_register_stamps_a_runs_roles_and_says_nothing_without_them() {
    let server = common::fleet(ROLE_LIST, ROLE_DETAIL);
    let model = model_from(server.port);

    let mut ui = simulator(view::view(&model));
    assert!(ui.find(says("role")).is_ok(), "the register names the column");
    // One run of three distinct roles is a count, not three rows: the run is
    // one run however many agents it boots.
    assert!(
        ui.find(says("3 ROLES")).is_ok(),
        "the role fleet's run is stamped with its role count"
    );
    // The ordinary run next to it carries no role, so it is stamped with
    // nothing at all.
    assert!(
        ui.find(says("r2")).is_ok(),
        "the ordinary run is still in the register"
    );

    // A run booted as one role is stamped with that role.
    let mut single = model.clone();
    single.runs[0].vms[1].role = None;
    single.runs[0].vms[2].role = None;
    let mut ui = simulator(view::view(&single));
    assert!(
        ui.find(says("BUILDER")).is_ok(),
        "a one-role run is stamped with the role itself"
    );
    assert!(
        ui.find(says("3 ROLES")).is_err(),
        "and not with a count of one"
    );

    // A register with no role at all stamps no role anywhere.
    let no_roles = common::fleet(LIST, DETAIL);
    let plain = model_from(no_roles.port);
    let mut ui = simulator(view::view(&plain));
    assert!(
        ui.find(says("BUILDER")).is_err(),
        "no role was reported, so no role is drawn"
    );
    assert!(ui.find(says("roles")).is_err());
}

#[test]
fn a_role_agents_row_in_the_run_pane_carries_its_role() {
    let server = common::fleet(ROLE_LIST, ROLE_DETAIL);
    let mut model = model_from(server.port);
    let client = Client::new(model.endpoint.clone());
    for message in [Message::Show(Pane::Run("r1".to_string()))] {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }
    assert_eq!(model.detail.as_ref().expect("the run").vms.len(), 3);

    let mut ui = simulator(view::view(&model));
    for expected in [
        "role",
        "BUILDER",
        "REVIEWER",
        "LEGAL-GOV-PROSECUTOR",
        "cid 100",
        "cid 101",
        "cid 102",
        // The agents' own serial is what the pane is for.
        "agent-harness: role builder accepted",
    ] {
        assert!(
            ui.find(says(expected)).is_ok(),
            "the run pane should show {expected:?}"
        );
    }
}

#[test]
fn the_roster_groups_who_is_running_by_role() {
    let server = common::fleet(ROLE_LIST, ROLE_DETAIL);
    let mut model = model_from(server.port);
    let client = Client::new(model.endpoint.clone());
    let effect = model.apply(Message::Show(Pane::Roster));
    assert_eq!(
        effect,
        Effect::roster(vec!["r1".to_string(), "r2".to_string()]),
        "the roster reads the register and every run's serial"
    );
    perform(effect, &mut model, &client);

    {
        let mut ui = simulator(view::view(&model));
        for expected in [
            "Roster",
            "who is running right now",
            // The tally is the answer to the pane's own question.
            "3 agent(s) · 2 up right now · 3 role(s)",
            // Every role the register reports, its own group, and its own agents.
            "builder",
            "reviewer",
            "legal-gov-prosecutor",
            "1 agent(s) · 1 up",
            "cid 100",
            "cid 101",
            "cid 102",
            "guest cid",
            "lines/s",
            "EXITED",
        ] {
            assert!(
                ui.find(says(expected)).is_ok(),
                "the roster should show {expected:?}"
            );
        }
        // The role's name is printed in full beside its stamp, so a long role
        // is never a mystery.
        assert!(ui.find(says("LEGAL-GOV-PROSECUTOR")).is_ok());
        // A VM with no role is not an agent: the demo run's VM (cid 50) is
        // not on the roster, whatever the index lists.
        assert!(
            ui.find(says("cid 50")).is_err(),
            "a run whose VMs carry no role has no roster row"
        );
    }

    let frame = frame_of(model);
    assert_eq!((frame.size.width, frame.size.height), (1280, 860));
    assert!(
        holds(&frame, OK, 8),
        "the two up agents are stamped in the register's green"
    );
}

#[test]
fn the_roster_says_so_when_no_vm_carries_a_role() {
    // A daemon that answered, and stamps no role on any VM row.
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    model.pane = Pane::Roster;
    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find(says("no role agent is in the register")).is_ok(),
        "the roster's empty state is the honest one: no role was reported"
    );
    // No tally of a fleet that is not there, and the index says so too.
    assert!(ui.find(says("up right now")).is_err());
    assert!(ui.find(says("no role agent yet")).is_ok());

    // And before the daemon has answered at all, it says that instead.
    let mut cold = Model::new();
    cold.pane = Pane::Roster;
    let mut ui = simulator(view::view(&cold));
    assert!(
        ui.find(says("waiting for the daemon's first answer")).is_ok(),
        "a register nobody has polled yet has nothing to name"
    );
}

#[test]
fn clicking_a_roster_row_opens_its_run_with_that_vm_selected() {
    let server = common::fleet(ROLE_LIST, ROLE_DETAIL);
    let mut model = model_from(server.port);
    let client = Client::new(model.endpoint.clone());
    model.pane = Pane::Roster;

    let mut ui = simulator(view::view(&model));
    ui.click("cid 101").expect("the reviewer's row is clickable");
    let messages: Vec<Message> = ui.into_messages().collect();
    assert!(
        messages.contains(&Message::Select {
            run: "r1".to_string(),
            vm: 1,
        }),
        "pressing an agent's row aims the console at that agent, got {messages:?}"
    );
    for message in messages {
        let effect = model.apply(message);
        perform(effect, &mut model, &client);
    }
    assert_eq!(model.pane, Pane::Run("r1".to_string()));
    assert_eq!(model.selection, Some(Selection { run: "r1".to_string(), vm: 1 }));

    let mut ui = simulator(view::view(&model));
    assert!(
        ui.find(says("agent-harness: role reviewer accepted")).is_ok(),
        "the reviewer's own serial is what the run pane opened on"
    );
}

#[test]
fn the_roster_pane_is_offered_by_the_index_and_the_key_sheet() {
    let server = common::fleet(ROLE_LIST, ROLE_DETAIL);
    let mut model = model_from(server.port);
    model.pane = Pane::Roster;

    let mut ui = simulator(view::view(&model));
    assert!(ui.find(says("Roster")).is_ok(), "the index lists the roster");
    assert!(ui.find(says("w")).is_ok(), "and names its key");
    assert!(
        ui.find(says("who is running")).is_ok(),
        "the index says what the roster answers"
    );
}

#[test]
fn a_fleet_run_running_for_days_reads_as_days_and_a_day_stamp() {
    // The register's own clock is the poll's: three days and seven hours into
    // a run that is still `running`.
    let server = common::fleet(LIST, DETAIL);
    let mut model = model_from(server.port);
    const DAY: u64 = 86_400_000;
    let now = 1_760_000_000_000u64;
    let age = 3 * DAY + 7 * 3_600_000 + 5 * 60_000;
    model.now_ms = now;
    model.runs[0].created_at = now - age;
    assert_eq!(model.run_elapsed_ms(&model.runs[0]), age);

    {
        let mut ui = simulator(view::view(&model));
        // Three days old is three days, not seventy-nine hours.
        assert!(
            ui.find(says("3d 07h")).is_ok(),
            "a run of days reads in days"
        );
        assert!(
            ui.find(says("79h")).is_err(),
            "an hour count must not run on past a day"
        );
        // Its creation is on another UTC day, so the register writes that day
        // rather than a bare time of day that would read as today's.
        assert!(
            ui.find(says("2025-10-06")).is_ok(),
            "a creation that is not today is stamped with its day"
        );
        assert!(
            ui.find(says("01:48:20 UTC")).is_err(),
            "and not with a time of day that names no day"
        );
    }

    // The same run, ended, keeps its own day for the age it lived.
    model.runs[0].state = agentplaza_desktop::api::RunState::Killed;
    model.runs[0].ended_at = Some(now);
    let mut ui = simulator(view::view(&model));
    assert!(ui.find(says("3d 07h")).is_ok());
    assert!(ui.find(says("2025-10-06")).is_ok(), "the day it began is kept");
}
