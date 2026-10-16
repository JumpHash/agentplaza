//! Real frames, drawn offscreen on a wgpu device: the Machines view of the 15-machine fixture (the fleet animation and the
//! GPU hardware animation on), the plaza, and the robot lab. Each must pass wgpu validation and show a picture that is not
//! flat, blank or blown out. Skipped where there is no adapter.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apz_engine::fleet::FleetResponse;
use apz_engine::stage::View;
use apz_engine::{Cmd, Engine, Feed};

const W: u32 = 320;
const H: u32 = 240;
/// Frames drawn per view, a few clock steps apart, so the animation runs while the last one is read back.
const FRAMES: u32 = 4;

/// Drives a future that is ready once the GPU has done its work (after a device poll), as the engine's own tests do.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::yield_now();
    }
}

/// A device for the test, or None on a machine with no adapter (the test is then skipped).
fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter =
        block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

fn fixture() -> FleetResponse {
    serde_json::from_str(include_str!("../testdata/floor-15.json")).expect("the fixture parses")
}

/// Draws `FRAMES` frames of `engine` into a fresh texture inside a validation error scope, and returns the last frame's
/// RGBA8 pixels, row by row from the top. `label` names the view in the failure messages.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, engine: &mut Engine, label: &str) -> Vec<u8> {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render_frames target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let t0 = Instant::now();
    for i in 0..FRAMES {
        let _ = engine.frame(
            t0 + Duration::from_millis(33 * u64::from(i)),
            &view,
            (W, H),
            1.0,
        );
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the device polls");
    let error = block_on(device.pop_error_scope());
    assert!(
        error.is_none(),
        "{label}: wgpu rejected the frame: {error:?}"
    );

    read_back(device, queue, &texture)
}

/// Copies the texture to a mapped buffer and returns its RGBA8 pixels without the row padding.
fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let row = (W * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("render_frames readback"),
        size: u64::from(row) * u64::from(H),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render_frames copy"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the device polls");
    rx.recv()
        .expect("the map callback ran")
        .expect("the readback maps");

    let mut out = Vec::with_capacity((W * H * 4) as usize);
    {
        let view = buffer.slice(..).get_mapped_range();
        for line in view.chunks(row as usize).take(H as usize) {
            out.extend_from_slice(&line[..(W * 4) as usize]);
        }
    }
    buffer.unmap();
    out
}

/// The picture is not uniform (more than 50 distinct colours) and not blank or blown out (its mean luminance is between
/// 2 and 253 of 255).
fn check_picture(label: &str, px: &[u8]) {
    assert_eq!(
        px.len(),
        (W * H * 4) as usize,
        "{label}: the readback is the asked size"
    );
    let distinct: HashSet<[u8; 4]> = px
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect();
    assert!(
        distinct.len() > 50,
        "{label}: only {} distinct colours, the frame is flat",
        distinct.len()
    );
    let luma: f64 = px
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| 0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2]))
        .sum::<f64>()
        / f64::from(W * H);
    assert!(
        luma > 2.0 && luma < 253.0,
        "{label}: mean luminance {luma:.1} of 255 (blank or blown out)"
    );
}

#[test]
// GPU_HW_ANIM is a const; the test pins it on, so the assert is deliberate.
#[allow(clippy::assertions_on_constants)]
fn real_frames_pass_wgpu_validation_and_show_a_picture() {
    assert!(
        apz_engine::scene::GPU_HW_ANIM,
        "the GPU hardware animation is on"
    );
    let Some((device, queue)) = test_device() else {
        eprintln!("no wgpu adapter here: the frame render test is skipped");
        return;
    };
    // Errors outside a scope (none expected) are recorded here instead of panicking inside wgpu.
    let uncaptured = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = uncaptured.clone();
    device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
        sink.lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(format!("{e}"))
    }));

    // 1. The Machines view of the fixture, with the fleet animation running.
    let mut engine = Engine::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
    engine.set_data(Feed {
        fleet: Some(fixture()),
        ..Feed::default()
    });
    engine.command(Cmd::ShowMachines);
    assert!(
        engine.stage.view == View::Machines,
        "the fixture shows the Machines view"
    );
    let machines = render(&device, &queue, &mut engine, "machines");
    check_picture("machines", &machines);

    // 2. The plaza: shown directly when the fleet cannot be read.
    let mut engine = Engine::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
    engine.set_data(Feed {
        fleet_error: Some("the fleet is unreachable".into()),
        ..Feed::default()
    });
    engine.stage.set_view(View::Plaza);
    let plaza = render(&device, &queue, &mut engine, "plaza");
    check_picture("plaza", &plaza);

    // 3. The robot lab: APZ_ROBOT_LAB=all lines up every species on the floor.
    // SAFETY: this is the only test in this binary, so no other thread reads or writes the environment.
    unsafe { std::env::set_var("APZ_ROBOT_LAB", "all") };
    let mut engine = Engine::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
    engine.set_data(Feed {
        fleet: Some(fixture()),
        ..Feed::default()
    });
    unsafe { std::env::remove_var("APZ_ROBOT_LAB") };
    assert!(
        engine.stage.lab.is_some(),
        "the robot lab is reachable through APZ_ROBOT_LAB"
    );
    let lab = render(&device, &queue, &mut engine, "robot lab");
    check_picture("robot lab", &lab);

    let errors = uncaptured.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert!(
        errors.is_empty(),
        "wgpu reported errors outside the scopes: {errors:?}"
    );
}
