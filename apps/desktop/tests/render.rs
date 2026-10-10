//! Headless rendering: the register laid out and drawn for real, without a
//! window.
//!
//! iced's test harness drives the very same widgets and application the binary
//! runs, so these tests are the runtime verification for a host where a window
//! cannot be looked at: they assert what the sheet says, and what a click on
//! it produces, against a stub daemon over a real socket.

mod common;

use std::time::Duration;

use common::{DETAIL, LIST};

use agentplaza_desktop::api::{Client, Endpoint};
use agentplaza_desktop::cli::Config;
use agentplaza_desktop::model::{Effect, Message, Model, Pane};
use agentplaza_desktop::{app, theme, view};

use iced::Size;
use iced_test::simulator;

/// The page, as RGBA.
const PAPER: [u8; 4] = [0xe9, 0xec, 0xe5, 0xff];
/// The ink.
const INK: [u8; 4] = [0x15, 0x19, 0x16, 0xff];
/// The green a running state is stamped in.
const OK: [u8; 4] = [0x2e, 0x6a, 0x40, 0xff];
/// The amber a killed state is stamped in.
const WARN: [u8; 4] = [0x85, 0x64, 0x0e, 0xff];

/// The daemon stub's configuration.
fn config(port: u16) -> Config {
    Config {
        endpoint: Endpoint::loopback_at("127.0.0.1", port).expect("loopback"),
        timeout: Duration::from_secs(5),
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
    if let Some(id) = effect.fetch_run {
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
