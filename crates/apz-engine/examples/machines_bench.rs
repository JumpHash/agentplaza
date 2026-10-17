//! The Machines view's frame cost by fleet size: the 15-machine fixture repeated to N machines, timed offscreen.
//! `cargo run --release -p apz-engine --example machines_bench [N...]` prints, per N, the stage's step (advance) and
//! the whole frame (step, prepare, draw, submit and the GPU's wait) in ms, median over the frames after a warm-up.

use std::time::{Duration, Instant};

use apz_engine::fleet::FleetResponse;
use apz_engine::input::{Button, Event};
use apz_engine::render::MESH_UPLOAD_BYTES;
use apz_engine::{Engine, Feed, agent_list};

const W: u32 = 1280;
const H: u32 = 800;

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

fn fleet(n: usize) -> FleetResponse {
    let base: FleetResponse = serde_json::from_str(include_str!("../testdata/floor-15.json"))
        .expect("the fixture parses");
    let mut out = base.clone();
    out.machines.clear();
    for i in 0..n {
        let mut m = base.machines[i % base.machines.len()].clone();
        if i >= base.machines.len() {
            m.id = format!("{}-{i}", m.id);
            m.name = format!("{}{i}", m.name);
            m.role = "node".into();
        }
        out.machines.push(m);
    }
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        vec![1, 15, 30, 60]
    } else {
        sizes
    };
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("an adapter");
    println!("adapter: {:?}", adapter.get_info().name);
    let (device, queue) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    // APZ_BENCH_AGENTS=path: the hub's agent list (or a fixture holding one under "agents"), as the hosts feed it.
    let agents = std::env::var("APZ_BENCH_AGENTS").ok().map(|p| {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(p).expect("the agents file reads"))
                .expect("the agents file parses");
        agent_list(&v)
    });
    println!("agents: {}", agents.as_ref().map_or(0, Vec::len));
    for n in sizes {
        let mut engine = Engine::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let t_build = Instant::now();
        engine.set_data(Feed {
            fleet: Some(fleet(n)),
            agents: agents.clone(),
            ..Feed::default()
        });
        let build = t_build.elapsed().as_secs_f64() * 1e3;
        let t0 = Instant::now();
        let (mut step, mut whole) = (Vec::new(), Vec::new());
        let drag = std::env::var("APZ_BENCH_DRAG").is_ok();
        let up0 = MESH_UPLOAD_BYTES.load(std::sync::atomic::Ordering::Relaxed);
        for i in 0..120u64 {
            let now = t0 + Duration::from_millis(16 * i);
            if drag && i == 20 {
                engine.input(Event::Pressed {
                    button: Button::Left,
                    at: Some((600.0, 400.0)),
                });
            }
            if drag && i > 20 {
                let p = (
                    600.0 + (i as f32 * 3.0) % 200.0,
                    400.0 + (i as f32 / 10.0).sin() * 40.0,
                );
                engine.input(Event::Moved { at: Some(p), to: p });
            }
            let a = Instant::now();
            let _ = engine.advance(now, (W as f32, H as f32));
            let s = a.elapsed().as_secs_f64() * 1e3;
            let b = Instant::now();
            let _ = engine.frame(now + Duration::from_millis(8), &view, (W, H), 1.0);
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("the device polls");
            let f = b.elapsed().as_secs_f64() * 1e3;
            if i >= 20 {
                step.push(s);
                whole.push(f);
            }
        }
        let up = (MESH_UPLOAD_BYTES.load(std::sync::atomic::Ordering::Relaxed) - up0) as f64
            / 120.0
            / 1e6;
        println!(
            "{n:3} machines: set_data {build:7.1} ms | step {:6.2} ms | frame {:6.2} ms | uploads {up:6.2} MB/frame | draws {} {:?}",
            median(step.clone()),
            median(whole.clone()),
            engine.draw_calls(),
            engine.draw_breakdown()
        );
        println!(
            "      worst step {:6.2} ms | worst frame {:6.2} ms",
            step.iter().copied().fold(0.0, f64::max),
            whole.iter().copied().fold(0.0, f64::max)
        );
    }
}
