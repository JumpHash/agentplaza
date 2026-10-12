//! The Machines screen (parity with the web console's `machines` view): the control plane wired to
//! each machine, then a section per machine with its live usage, where its resources go, its runs
//! (serial, kill) and its technical details, folded away.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::canvas::{self, Path, Stroke};
use iced::widget::{column, container, mouse_area, row, scrollable, Space};
use iced::{mouse, Alignment, Border, Color, Element, Length, Pixels, Point, Rectangle, Task, Theme};
use serde_json::Value;

use super::Id;
use crate::console::ui::{Kind, Tone, INK, INK_2, INK_3, RULE, RULE_FAINT, SHEET, SIGNAL};
use crate::console::{client, ui, Ctx};
use crate::theme::MONO;

/// A machine's row in the topology, and the topology's fleet column.
const ROW_H: f32 = 56.0;
const FLEET_W: f32 = 260.0;
const PLANE_W: f32 = 200.0;
/// The CPU sparkline: its height, and how many of the latest samples it draws.
const SPARK_H: f32 = 72.0;
const SPARK_POINTS: usize = 60;

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    /// The last good read of the fleet; None before the first load and while the node is offline.
    data: Option<Data>,
    /// Why the last read failed, while the node is offline.
    offline: Option<String>,
    /// The machine the topology picked.
    selected: Option<String>,
    /// Each machine's Agents-table sort, kept across refreshes.
    sorts: HashMap<String, Sort>,
    /// Runs with their VMs open; `run/index` serials open; machines with technical details open.
    expanded: BTreeSet<String>,
    serial_open: BTreeSet<String>,
    tech_open: BTreeSet<String>,
    /// The VMs last read for each open run, so a refresh does not blank them.
    run_vms: HashMap<String, Result<Vec<VmRow>, String>>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Value, String>),
    /// The topology picked a machine.
    Pick(String),
    Sort(String, SortKey),
    ToggleRun(String),
    RunLoaded(String, Result<Value, String>),
    ToggleSerial(String),
    ToggleTech(String),
    Kill(String),
    Killed(Result<(), String>),
}

/// The Agents table's columns that sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Mem,
}

#[derive(Debug, Clone, Copy)]
struct Sort {
    key: SortKey,
    descending: bool,
}

const DEFAULT_SORT: Sort = Sort { key: SortKey::Cpu, descending: true };

#[derive(Debug, Clone)]
struct Data {
    overview: Option<Overview>,
    machines: Vec<Machine>,
    /// Agents by CID, for the names of the VMs.
    agents: HashMap<u64, AgentRef>,
}

#[derive(Debug, Clone)]
struct Overview {
    total: u64,
    working: u64,
    hub: bool,
}

#[derive(Debug, Clone)]
struct AgentRef {
    name: String,
    role_label: String,
}

#[derive(Debug, Clone)]
struct Machine {
    id: String,
    name: String,
    kind: String,
    class: String,
    arch: String,
    backend: String,
    irqchip: String,
    vsock_mode: String,
    noded_port: Option<u64>,
    cpus: u64,
    memory_mib: f64,
    gpus: Vec<String>,
    runs: Vec<Run>,
    agents: u64,
    reachable: Option<bool>,
    stats: Option<Stats>,
}

#[derive(Debug, Clone)]
struct Stats {
    cpu: f64,
    load: [f64; 3],
    mem_total: f64,
    mem_used: f64,
    uptime_s: f64,
    vms_running: u64,
    guest_mem: f64,
    history: Vec<f64>,
    plaza: Option<Plaza>,
}

#[derive(Debug, Clone)]
struct Plaza {
    services: Vec<Proc>,
    vms: Vec<VmUsage>,
    total: Total,
}

#[derive(Debug, Clone)]
struct Total {
    services_cpu: f64,
    services_rss: f64,
    vms_cpu: f64,
    vms_rss: f64,
}

/// A process of the plaza on a machine; `cpu` is of the whole machine.
#[derive(Debug, Clone)]
struct Proc {
    name: String,
    pid: u64,
    cpu: f64,
    rss: f64,
}

/// An agent VM's process, and its guest memory as the balloon reports it.
#[derive(Debug, Clone)]
struct VmUsage {
    run: String,
    vm: u64,
    cid: Option<u64>,
    role: Option<String>,
    cpu: f64,
    rss: f64,
    usable: Option<f64>,
    guest_used: Option<f64>,
}

#[derive(Debug, Clone)]
struct Run {
    id: String,
    state: String,
    role: Option<String>,
    created_at: f64,
    vms: Vec<VmRow>,
}

#[derive(Debug, Clone)]
struct VmRow {
    index: u64,
    state: String,
    guest_cid: Option<u64>,
    serial: Option<String>,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick): the overview, the
/// machines and the agents, in one task.
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            let overview = client.get::<Value>("/v1/overview").ok();
            let machines = client.get::<Value>("/v1/machines")?;
            let agents = client.get::<Value>("/v1/agents").ok();
            Ok(serde_json::json!({ "overview": overview, "machines": machines, "agents": agents }))
        },
        Msg::Loaded,
    )
}

/// Reads one run with its VMs (the serial lines come with them).
fn fetch_run(ctx: &Ctx, id: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/runs/{id}");
    let key = id.clone();
    client::task(move || client.get::<Value>(&path), move |r| Msg::RunLoaded(key.clone(), r))
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(v)) => {
            let data = parse_data(&v);
            let run_ids: BTreeSet<String> = data.machines.iter().flat_map(|m| m.runs.iter().map(|r| r.id.clone())).collect();
            state.expanded.retain(|id| run_ids.contains(id));
            if !data.machines.iter().any(|m| Some(&m.id) == state.selected.as_ref()) {
                state.selected = data.machines.first().map(|m| m.id.clone());
            }
            state.offline = None;
            let fetches: Vec<Task<Msg>> = state.expanded.iter().map(|id| fetch_run(ctx, id.clone())).collect();
            state.data = Some(data);
            Task::batch(fetches)
        }
        Msg::Loaded(Err(e)) => {
            state.data = None;
            state.offline = Some(e);
            Task::none()
        }
        Msg::Pick(id) => {
            state.selected = Some(id);
            Task::none()
        }
        Msg::Sort(id, key) => {
            let cur = state.sorts.get(&id).copied().unwrap_or(DEFAULT_SORT);
            let next = if cur.key == key { Sort { key, descending: !cur.descending } } else { Sort { key, descending: true } };
            state.sorts.insert(id, next);
            Task::none()
        }
        Msg::ToggleRun(id) => {
            if state.expanded.remove(&id) {
                Task::none()
            } else {
                state.expanded.insert(id.clone());
                fetch_run(ctx, id)
            }
        }
        Msg::RunLoaded(id, r) => {
            if state.expanded.contains(&id) {
                let vms: Result<Vec<VmRow>, String> = r.map(|v| items(&v, "vms").iter().map(parse_vm_row).collect());
                state.run_vms.insert(id, vms);
            }
            Task::none()
        }
        Msg::ToggleSerial(key) => {
            toggle(&mut state.serial_open, key);
            Task::none()
        }
        Msg::ToggleTech(id) => {
            toggle(&mut state.tech_open, id);
            Task::none()
        }
        Msg::Kill(id) => {
            let client = ctx.client.clone();
            client::task(move || client.call("DELETE", &format!("/v1/runs/{id}"), None).map(|_| ()), Msg::Killed)
        }
        Msg::Killed(Ok(())) => load(state, ctx),
        Msg::Killed(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
    }
}

fn toggle(set: &mut BTreeSet<String>, key: String) {
    if !set.remove(&key) {
        set.insert(key);
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    ui::sheet(ui::view_head("Machines", vec![]), body(state))
}

fn body<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    match &state.data {
        None if state.offline.is_some() => {
            parts.push(ui::empty("machine", "The node is offline", "Start the node daemon to see its machines and VMs."))
        }
        None => parts.push(ui::loading()),
        Some(data) => {
            parts.push(topology(state, data));
            if data.machines.is_empty() {
                parts.push(ui::empty("machine", "No machines.", "The plaza has no machines to show."));
            } else {
                for m in &data.machines {
                    parts.push(section(state, data, m));
                }
            }
        }
    }
    column(parts).spacing(32).into()
}

// ---- The topology: the control plane, wired to each machine ---------------------------------

fn topology<'a>(state: &'a State, data: &'a Data) -> Element<'a, Msg> {
    let fleet_h = data.machines.len().max(1) as f32 * ROW_H;
    let wires = iced::widget::canvas(Wires {
        live: data.machines.iter().map(|m| is_live(&m.runs)).collect(),
        labels: data.machines.iter().map(|m| format!("{} agents · {} VMs", m.agents, machine_vms(m))).collect(),
    })
    .width(Length::Fill)
    .height(Length::Fixed(fleet_h));

    let blocks: Vec<Element<'a, Msg>> = data.machines.iter().map(|m| block(state, m)).collect();
    let fleet = column(blocks).width(Length::Fixed(FLEET_W));

    let overview_text = match &data.overview {
        Some(o) => format!("{} agents · {} working", o.total, o.working),
        None => "Unreachable".to_string(),
    };
    let hub = data.overview.as_ref().is_some_and(|o| o.hub);
    let plane = container(
        column![
            ui::label("Control plane"),
            ui::faint(overview_text),
            row![
                ui::badge("apid", if data.overview.is_some() { Tone::Ok } else { Tone::Danger }),
                ui::badge("hub", if hub { Tone::Ok } else { Tone::Muted }),
            ]
            .spacing(8),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Length::Fixed(PLANE_W))
    .style(boxed);

    row![plane, wires, fleet].align_y(Alignment::Center).into()
}

/// The straight rules from the control plane's middle to each machine's row, drawn over the
/// topology; a live machine's rule is the signal orange, the rest are muted.
struct Wires {
    live: Vec<bool>,
    labels: Vec<String>,
}

impl canvas::Program<Msg> for Wires {
    type State = ();

    fn draw(&self, _state: &(), renderer: &iced::Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let (w, h) = (bounds.width, bounds.height);
        let rows = self.live.len().max(1) as f32;
        let hub = Point::new(0.0, h / 2.0);
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        for (i, (live, label)) in self.live.iter().zip(self.labels.iter()).enumerate() {
            let end = Point::new(w, (i as f32 + 0.5) * h / rows);
            let (color, width) = if *live { (SIGNAL, 2.0) } else { (INK_3, 1.0) };
            frame.stroke(&Path::line(hub, end), Stroke::default().with_color(color).with_width(width));
            frame.fill_text(canvas::Text {
                content: label.clone(),
                position: Point::new(w * 0.5, (hub.y + end.y) * 0.5 - 14.0),
                color: INK_2,
                size: Pixels(11.0),
                font: MONO,
                ..canvas::Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

/// One machine's block in the fleet column: its name, its kind and state. Picking it selects it.
fn block<'a>(state: &'a State, m: &'a Machine) -> Element<'a, Msg> {
    let live = is_live(&m.runs);
    let down = m.reachable == Some(false);
    let tone = if down {
        Tone::Danger
    } else if live {
        Tone::Ok
    } else {
        Tone::Muted
    };
    let status = if down {
        "unreachable"
    } else if live {
        "running"
    } else {
        "idle"
    };
    let selected = state.selected.as_deref() == Some(m.id.as_str());
    let content = column![
        row![swatch(tone.color(), 8.0), ui::mono(m.name.clone(), 13.0, INK)].spacing(8).align_y(Alignment::Center),
        ui::faint(format!("{} · {status}", kind_label(&m.kind))),
    ]
    .spacing(4);
    let cell = container(content)
        .padding([8, 12])
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .align_y(Alignment::Center)
        .style(block_style(selected));
    mouse_area(cell).on_press(Msg::Pick(m.id.clone())).into()
}

fn block_style(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |_: &Theme| container::Style {
        background: if selected { Some(SHEET.into()) } else { None },
        border: Border { color: if selected { INK } else { RULE }, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

// ---- A machine's section ----------------------------------------------------------------------

fn section<'a>(state: &'a State, data: &'a Data, m: &'a Machine) -> Element<'a, Msg> {
    let figure: Element<'a, Msg> = container(ui::icon("machine", 64.0, INK_3))
        .width(Length::Fixed(180.0))
        .height(Length::Fixed(160.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(boxed)
        .into();
    let parts: Vec<Element<'a, Msg>> = vec![
        ui::mono(m.name.clone(), 18.0, INK).into(),
        ui::faint(sentence(m)).into(),
        live_stats(m),
        resources(state, data, m),
        ui::label("Runs").into(),
        runs(state, m),
        technical(state, m),
    ];
    row![figure, column(parts).spacing(16).width(Length::Fill)].spacing(24).into()
}

/// The machine's tallies: CPU with its sparkline, memory, load and the agents' VMs.
fn live_stats<'a>(m: &'a Machine) -> Element<'a, Msg> {
    let Some(s) = &m.stats else {
        return ui::faint("No live stats.").into();
    };
    let tallies = row![
        tally("CPU", format!("{}%", s.cpu.round()), None, Some(spark(&s.history))),
        tally(
            "Memory",
            format!("{} / {}", gib(s.mem_used), gib(s.mem_total)),
            Some("GiB used of GiB".to_string()),
            Some(stacked(&[(s.mem_used, INK), ((s.mem_total - s.mem_used).max(0.0), RULE_FAINT)], s.mem_total, 6.0)),
        ),
        tally(
            "Load",
            format!("{:.1} · {:.1} · {:.1}", s.load[0], s.load[1], s.load[2]),
            Some("1 / 5 / 15 min".to_string()),
            None,
        ),
        tally("Agents' VMs", s.vms_running.to_string(), Some(format!("{} GiB reserved", gib(s.guest_mem))), None),
    ]
    .spacing(12);
    column![tallies, ui::faint(format!("Up {}", uptime(s.uptime_s)))].spacing(12).into()
}

fn tally<'a>(label: &str, figure: String, caption: Option<String>, extra: Option<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::label(label).into(), ui::mono(figure, 20.0, INK).into()];
    if let Some(extra) = extra {
        parts.push(extra);
    }
    if let Some(caption) = caption {
        parts.push(ui::faint(caption).into());
    }
    container(column(parts).spacing(8)).padding(12).width(Length::FillPortion(1)).style(boxed).into()
}

/// The last 60 CPU samples as one ink line over a ruled baseline.
struct Spark(Vec<f64>);

impl canvas::Program<Msg> for Spark {
    type State = ();

    fn draw(&self, _state: &(), renderer: &iced::Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let (w, h) = (bounds.width, bounds.height);
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.stroke(&Path::line(Point::new(0.0, h - 0.5), Point::new(w, h - 0.5)), Stroke::default().with_color(RULE).with_width(1.0));
        let pts = &self.0[self.0.len().saturating_sub(SPARK_POINTS)..];
        if pts.len() >= 2 {
            let steps = (pts.len() - 1) as f32;
            let path = Path::new(|b| {
                for (i, cpu) in pts.iter().enumerate() {
                    let x = i as f32 / steps * w;
                    let y = h - 4.0 - (cpu.clamp(0.0, 100.0) as f32 / 100.0) * (h - 8.0);
                    if i == 0 {
                        b.move_to(Point::new(x, y));
                    } else {
                        b.line_to(Point::new(x, y));
                    }
                }
            });
            frame.stroke(&path, Stroke::default().with_color(INK).with_width(2.0));
        }
        vec![frame.into_geometry()]
    }
}

fn spark<'a>(history: &[f64]) -> Element<'a, Msg> {
    iced::widget::canvas(Spark(history.to_vec())).width(Length::Fill).height(Length::Fixed(SPARK_H)).into()
}

/// A bar split into segments, each a share of `span`; the rest of the bar stays empty.
fn stacked<'a>(segments: &[(f64, Color)], span: f64, height: f32) -> Element<'a, Msg> {
    let mut children: Vec<Element<'a, Msg>> = Vec::new();
    for &(value, color) in segments {
        let portion = (value / span.max(f64::MIN_POSITIVE) * 1000.0).round().clamp(0.0, 1000.0) as u16;
        if portion == 0 {
            continue;
        }
        let cell = container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::FillPortion(portion))
            .height(Length::Fill)
            .style(move |_: &Theme| container::Style { background: Some(color.into()), ..Default::default() });
        children.push(cell.into());
    }
    row(children).width(Length::Fill).height(Length::Fixed(height)).into()
}

/// A small square in a colour, for legends and status marks.
fn swatch<'a>(color: Color, size: f32) -> Element<'a, Msg> {
    container(Space::new().width(Length::Fixed(size)).height(Length::Fixed(size)))
        .style(move |_: &Theme| container::Style { background: Some(color.into()), ..Default::default() })
        .into()
}

/// Where the machine's resources go: CPU and memory split four ways (services, agents, the rest
/// of the system, free), then the plaza's services and the agents' VMs.
fn resources<'a>(state: &'a State, data: &'a Data, m: &'a Machine) -> Element<'a, Msg> {
    let heading = ui::label("Where the resources go");
    let Some(stats) = &m.stats else {
        return column![heading, ui::faint("This node doesn't report a breakdown yet.")].spacing(8).into();
    };
    let Some(plaza) = &stats.plaza else {
        return column![heading, ui::faint("This node doesn't report a breakdown yet.")].spacing(8).into();
    };
    let t = &plaza.total;
    let cpu_parts = [
        ("Plaza services", t.services_cpu, INK),
        ("Agents", t.vms_cpu, SIGNAL),
        ("Rest of the system", (stats.cpu - (t.services_cpu + t.vms_cpu)).max(0.0), INK_3),
        ("Free", (100.0 - stats.cpu).max(0.0), RULE_FAINT),
    ];
    let mem_parts = [
        ("Plaza services", t.services_rss, INK),
        ("Agents", t.vms_rss, SIGNAL),
        ("Rest of the system", (stats.mem_used - (t.services_rss + t.vms_rss)).max(0.0), INK_3),
        ("Free", (stats.mem_total - stats.mem_used).max(0.0), RULE_FAINT),
    ];

    // The services table.
    let mut service_lines: Vec<Element<'a, Msg>> = vec![
        grid(vec![
            (4, false, ui::label("Service").into()),
            (2, true, ui::label("PID").into()),
            (2, true, ui::label("CPU").into()),
            (2, true, ui::label("Memory").into()),
        ]),
        ui::hairline(),
    ];
    for p in &plaza.services {
        service_lines.push(grid(vec![
            (4, false, ui::faint(p.name.clone()).into()),
            (2, true, ui::mono(p.pid.to_string(), 12.0, INK_2).into()),
            (2, true, ui::mono(pct(p.cpu), 12.0, INK).into()),
            (2, true, ui::mono(human(p.rss), 12.0, INK).into()),
        ]));
        service_lines.push(ui::hairline());
    }

    // The agents' table, sorted as the machine's sort says.
    let sort = state.sorts.get(&m.id).copied().unwrap_or(DEFAULT_SORT);
    let mut vms: Vec<&VmUsage> = plaza.vms.iter().collect();
    vms.sort_by(|a, b| {
        let (x, y) = match sort.key {
            SortKey::Cpu => (a.cpu, b.cpu),
            SortKey::Mem => (a.rss, b.rss),
        };
        let order = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
        if sort.descending {
            order.reverse()
        } else {
            order
        }
    });
    let agents_block: Element<'a, Msg> = if plaza.vms.is_empty() {
        ui::faint("No agent VMs on this node.").into()
    } else {
        let head = |label: &str, key: SortKey| -> Element<'a, Msg> {
            let active = sort.key == key;
            let text = if active {
                format!("{label} {}", if sort.descending { "↓" } else { "↑" })
            } else {
                label.to_string()
            };
            ui::btn(&text, None, Kind::Ghost, Some(Msg::Sort(m.id.clone(), key)))
        };
        let mut lines: Vec<Element<'a, Msg>> = vec![
            grid(vec![
                (3, false, ui::label("Agent").into()),
                (2, false, ui::label("VM").into()),
                (2, true, head("CPU", SortKey::Cpu)),
                (2, true, head("Host memory", SortKey::Mem)),
                (3, true, ui::label("Guest used / usable").into()),
            ]),
            ui::hairline(),
        ];
        for v in &vms {
            let agent = v.cid.and_then(|c| data.agents.get(&c));
            let name = match (agent, v.cid) {
                (Some(a), _) if a.name.trim().is_empty() => "Unnamed".to_string(),
                (Some(a), _) => a.name.clone(),
                (None, None) => "Unattached".to_string(),
                (None, Some(c)) => format!("CID {c}"),
            };
            let role = agent.map(|a| a.role_label.clone()).or_else(|| v.role.clone()).unwrap_or_else(|| "—".to_string());
            let who: Element<'a, Msg> = column![ui::mono(name, 12.0, INK), ui::faint(role)].spacing(2).into();
            lines.push(grid(vec![
                (3, false, who),
                (2, false, ui::mono(format!("{}/{}", v.run, v.vm), 12.0, INK_2).into()),
                (2, true, ui::mono(pct(v.cpu), 12.0, INK).into()),
                (2, true, ui::mono(human(v.rss), 12.0, INK).into()),
                (3, true, guest_cell(v)),
            ]));
            lines.push(ui::hairline());
        }
        let sum_cpu: f64 = plaza.vms.iter().map(|v| v.cpu).sum();
        let sum_rss: f64 = plaza.vms.iter().map(|v| v.rss).sum();
        let guest_total = if plaza.vms.iter().any(|v| v.guest_used.is_some()) {
            let used: f64 = plaza.vms.iter().filter_map(|v| v.guest_used).sum();
            let usable: f64 = plaza.vms.iter().filter_map(|v| v.usable).sum();
            format!("{} / {}", human(used), human(usable))
        } else {
            "—".to_string()
        };
        lines.push(grid(vec![
            (3, false, ui::mono(format!("{} VMs", plaza.vms.len()), 12.0, INK).into()),
            (2, false, Space::new().into()),
            (2, true, ui::mono(pct(sum_cpu), 12.0, INK).into()),
            (2, true, ui::mono(human(sum_rss), 12.0, INK).into()),
            (3, true, ui::mono(guest_total, 12.0, INK).into()),
        ]));
        column(lines).into()
    };

    column![
        heading,
        breakdown("CPU", &cpu_parts, 100.0, pct),
        breakdown("Memory", &mem_parts, stats.mem_total, gib_text),
        ui::label("Services"),
        column(service_lines),
        ui::label("Agents"),
        agents_block,
    ]
    .spacing(12)
    .into()
}

/// One stacked bar with its legend: each part's share of the bar, and the bar is at least `whole`.
fn breakdown<'a>(title: &str, parts: &[(&str, f64, Color)], whole: f64, fmt: fn(f64) -> String) -> Element<'a, Msg> {
    let sum: f64 = parts.iter().map(|p| p.1).sum();
    let span = whole.max(sum).max(1.0);
    let segments: Vec<(f64, Color)> = parts.iter().map(|&(_, value, color)| (value, color)).collect();
    let mut legend: Vec<Element<'a, Msg>> = Vec::new();
    for &(label, value, color) in parts {
        legend.push(
            row![swatch(color, 8.0), ui::faint(label.to_string()), ui::mono(fmt(value), 11.0, INK)]
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
        );
    }
    column![ui::faint(title.to_string()), stacked(&segments, span, 10.0), row(legend).spacing(16)].spacing(8).into()
}

/// A VM's guest memory as "used / usable", with a thin bar of the share used.
fn guest_cell<'a>(v: &VmUsage) -> Element<'a, Msg> {
    let used = v.guest_used.map_or("—".to_string(), human);
    let usable = v.usable.map_or("—".to_string(), human);
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::mono(format!("{used} / {usable}"), 12.0, INK).into()];
    if let (Some(used), Some(usable)) = (v.guest_used, v.usable) {
        if usable > 0.0 {
            parts.push(stacked(&[(used.min(usable), INK_2), ((usable - used).max(0.0), RULE_FAINT)], usable, 4.0));
        }
    }
    column(parts).spacing(4).into()
}

/// One table line: cells of a portion each, right-aligned when `end`.
fn grid<'a>(cells: Vec<(u16, bool, Element<'a, Msg>)>) -> Element<'a, Msg> {
    let children: Vec<Element<'a, Msg>> = cells
        .into_iter()
        .map(|(portion, end, content)| {
            container(content)
                .width(Length::FillPortion(portion))
                .align_x(if end { Alignment::End } else { Alignment::Start })
                .into()
        })
        .collect();
    row(children).spacing(12).padding([8, 12]).align_y(Alignment::Center).width(Length::Fill).into()
}

/// The machine's runs: a row each, opened to its VMs when clicked, with a kill.
fn runs<'a>(state: &'a State, m: &'a Machine) -> Element<'a, Msg> {
    if m.runs.is_empty() {
        return ui::faint("No runs.").into();
    }
    let mut lines: Vec<Element<'a, Msg>> = vec![
        grid(vec![
            (3, false, ui::label("Run").into()),
            (2, false, ui::label("Role").into()),
            (2, false, ui::label("State").into()),
            (2, true, ui::label("VMs").into()),
            (2, true, ui::label("Started").into()),
            (2, true, Space::new().into()),
        ]),
        ui::hairline(),
    ];
    for run in &m.runs {
        let kill = ui::btn("Kill", Some("stop"), Kind::Danger, Some(Msg::Kill(run.id.clone())));
        let line = grid(vec![
            (3, false, ui::mono(run.id.clone(), 12.0, INK).into()),
            (2, false, ui::faint(run.role.clone().unwrap_or_else(|| "fleet".to_string())).into()),
            (2, false, ui::badge(&run.state, state_tone(&run.state))),
            (2, true, ui::faint(format!("{} VMs", run.vms.len())).into()),
            (2, true, ui::faint(format!("started {}", ago(run.created_at))).into()),
            (2, true, kill),
        ]);
        lines.push(mouse_area(line).on_press(Msg::ToggleRun(run.id.clone())).into());
        if state.expanded.contains(&run.id) {
            lines.push(vm_details(state, run));
        }
        lines.push(ui::hairline());
    }
    column(lines).into()
}

/// An open run's VMs, read on open and on every refresh.
fn vm_details<'a>(state: &'a State, run: &'a Run) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match state.run_vms.get(&run.id) {
        None => ui::faint("Loading…").into(),
        Some(Err(e)) => ui::faint(e.clone()).into(),
        Some(Ok(vms)) if vms.is_empty() => ui::faint("No VMs.").into(),
        Some(Ok(vms)) => column(vms.iter().map(|vm| vm_line(state, &run.id, vm)).collect::<Vec<_>>()).spacing(8).into(),
    };
    container(body).padding([8, 24]).width(Length::Fill).into()
}

fn vm_line<'a>(state: &'a State, run: &str, vm: &'a VmRow) -> Element<'a, Msg> {
    let key = format!("{run}/{}", vm.index);
    let guest = vm.guest_cid.map_or("—".to_string(), |c| c.to_string());
    let head: Element<'a, Msg> = row![
        ui::mono(format!("VM {}", vm.index), 12.0, INK),
        ui::badge(&vm.state, state_tone(&vm.state)),
        ui::faint(format!("guest CID {guest}")),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into();
    let mut lines: Vec<Element<'a, Msg>> = vec![head];
    if vm.serial.is_some() {
        let open = state.serial_open.contains(&key);
        lines.push(ui::btn(
            if open { "Serial ▾" } else { "Serial ▸" },
            None,
            Kind::Ghost,
            Some(Msg::ToggleSerial(key.clone())),
        ));
        if let (true, Some(serial)) = (open, &vm.serial) {
            lines.push(
                scrollable(container(ui::mono(serial.clone(), 11.0, INK_2)).padding(8).style(boxed))
                    .width(Length::Fill)
                    .into(),
            );
        }
    }
    column(lines).spacing(6).into()
}

/// The machine's technical details, folded away.
fn technical<'a>(state: &'a State, m: &'a Machine) -> Element<'a, Msg> {
    let open = state.tech_open.contains(&m.id);
    let toggle = ui::btn(
        if open { "Technical details ▾" } else { "Technical details ▸" },
        None,
        Kind::Ghost,
        Some(Msg::ToggleTech(m.id.clone())),
    );
    if !open {
        return toggle;
    }
    let port = m.noded_port.map_or("—".to_string(), |p| p.to_string());
    let rows = [
        ("Class", m.class.clone()),
        ("Arch", m.arch.clone()),
        ("Backend", m.backend.clone()),
        ("Irqchip", m.irqchip.clone()),
        ("vsock", m.vsock_mode.clone()),
        ("noded port", port),
    ];
    let mut lines: Vec<Element<'a, Msg>> = vec![toggle];
    for (key, value) in rows {
        lines.push(
            row![container(ui::faint(key.to_string())).width(Length::Fixed(160.0)), ui::mono(value, 12.0, INK)]
                .spacing(12)
                .into(),
        );
    }
    column(lines).spacing(8).into()
}

// ---- Helpers ----------------------------------------------------------------------------------

fn boxed(_: &Theme) -> container::Style {
    container::Style {
        border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

fn is_live(runs: &[Run]) -> bool {
    runs.iter().any(|r| r.vms.iter().any(|vm| state_tone(&vm.state) == Tone::Ok))
}

fn machine_vms(m: &Machine) -> usize {
    m.runs.iter().map(|r| r.vms.len()).sum()
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "server" => "Server",
        "workstation" => "Workstation",
        "edge" => "Edge",
        "apple" => "Apple",
        other => other,
    }
}

fn state_tone(state: &str) -> Tone {
    let s = state.to_lowercase();
    if ["run", "ready", "up", "active"].iter().any(|w| s.contains(*w)) {
        Tone::Ok
    } else if ["start", "boot", "pend", "creat", "launch"].iter().any(|w| s.contains(*w)) {
        Tone::Info
    } else if ["fail", "err", "crash", "kill", "dead"].iter().any(|w| s.contains(*w)) {
        Tone::Danger
    } else {
        Tone::Muted
    }
}

fn sentence(m: &Machine) -> String {
    let head = format!(
        "{} · {}-core {} on {} · {} GiB memory",
        kind_label(&m.kind),
        m.cpus,
        m.arch,
        m.backend.to_uppercase(),
        (m.memory_mib / 1024.0).round()
    );
    if m.gpus.is_empty() {
        head
    } else {
        format!("{head} · {}", m.gpus.join(", "))
    }
}

fn uptime(s: f64) -> String {
    if s < 60.0 {
        return format!("{}s", s.floor());
    }
    let mins = (s / 60.0).floor() as u64;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h {}m", mins % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}

fn gib(mib: f64) -> String {
    format!("{:.1}", mib / 1024.0)
}

fn gib_text(mib: f64) -> String {
    format!("{} GiB", gib(mib))
}

fn human(mib: f64) -> String {
    if mib >= 1024.0 {
        gib_text(mib)
    } else {
        format!("{} MiB", mib.round())
    }
}

fn pct(n: f64) -> String {
    format!("{n:.1}%")
}

/// A relative time since `ms` (as the web's `ago`); "—" for no time.
fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return "—".to_string();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
    let s = ((now - ms) / 1000.0).round().max(0.0) as u64;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

// ---- Reading the hub's JSON -------------------------------------------------------------------

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn opt_num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn opt_count(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

fn num_at(v: &Value, path: &str) -> f64 {
    v.pointer(path).and_then(Value::as_f64).unwrap_or(0.0)
}

fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn parse_data(v: &Value) -> Data {
    Data {
        overview: v.get("overview").filter(|o| o.is_object()).map(parse_overview),
        machines: items(v, "machines").iter().filter_map(parse_machine).collect(),
        agents: items(v, "agents").iter().filter_map(parse_agent).collect(),
    }
}

fn parse_overview(o: &Value) -> Overview {
    Overview {
        total: o.pointer("/agents/total").and_then(Value::as_u64).unwrap_or(0),
        working: o.pointer("/agents/working").and_then(Value::as_u64).unwrap_or(0),
        hub: o.get("hub").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn parse_agent(a: &Value) -> Option<(u64, AgentRef)> {
    let cid = a.get("cid").and_then(Value::as_u64)?;
    let role = text(a, "role");
    let label = text(a, "role_label");
    let role_label = if label.is_empty() { role } else { label };
    Some((cid, AgentRef { name: text(a, "name"), role_label }))
}

fn parse_machine(m: &Value) -> Option<Machine> {
    let id = text(m, "id");
    if id.is_empty() {
        return None;
    }
    Some(Machine {
        name: text(m, "name"),
        kind: text(m, "kind"),
        class: text(m, "class"),
        arch: text(m, "arch"),
        backend: text(m, "backend"),
        irqchip: text(m, "irqchip"),
        vsock_mode: text(m, "vsock_mode"),
        noded_port: opt_count(m, "noded_port"),
        cpus: count(m, "cpus"),
        memory_mib: num(m, "memory_mib"),
        gpus: items(m, "gpus").iter().map(|g| text(g, "name")).collect(),
        runs: items(m, "runs").iter().map(parse_run).collect(),
        agents: count(m, "agents"),
        reachable: m.get("reachable").and_then(Value::as_bool),
        stats: m.get("stats").filter(|s| s.is_object()).map(parse_stats),
        id,
    })
}

fn parse_stats(s: &Value) -> Stats {
    let load = items(s, "load");
    let at = |i: usize| load.get(i).and_then(Value::as_f64).unwrap_or(0.0);
    Stats {
        cpu: num(s, "cpu_percent"),
        load: [at(0), at(1), at(2)],
        mem_total: num(s, "mem_total_mib"),
        mem_used: num(s, "mem_used_mib"),
        uptime_s: num(s, "uptime_s"),
        vms_running: count(s, "vms_running"),
        guest_mem: num(s, "guest_mem_mib"),
        history: items(s, "history").iter().map(|p| num(p, "cpu_percent")).collect(),
        plaza: s.get("plaza").filter(|p| p.is_object()).map(parse_plaza),
    }
}

fn parse_plaza(p: &Value) -> Plaza {
    Plaza {
        services: items(p, "services").iter().map(parse_proc).collect(),
        vms: items(p, "vms").iter().map(parse_vm_usage).collect(),
        total: Total {
            services_cpu: num_at(p, "/total/services_cpu_percent"),
            services_rss: num_at(p, "/total/services_rss_mib"),
            vms_cpu: num_at(p, "/total/vms_cpu_percent"),
            vms_rss: num_at(p, "/total/vms_rss_mib"),
        },
    }
}

fn parse_proc(v: &Value) -> Proc {
    Proc { name: text(v, "name"), pid: count(v, "pid"), cpu: num(v, "cpu_percent"), rss: num(v, "rss_mib") }
}

fn parse_vm_usage(v: &Value) -> VmUsage {
    VmUsage {
        run: text(v, "run"),
        vm: count(v, "vm"),
        cid: opt_count(v, "cid"),
        role: opt_text(v, "role"),
        cpu: num(v, "cpu_percent"),
        rss: num(v, "rss_mib"),
        usable: opt_num(v, "usable_mib"),
        guest_used: opt_num(v, "guest_used_mib"),
    }
}

fn parse_run(r: &Value) -> Run {
    Run {
        id: text(r, "id"),
        state: text(r, "state"),
        role: opt_text(r, "role"),
        created_at: num(r, "created_at"),
        vms: items(r, "vms").iter().map(parse_vm_row).collect(),
    }
}

fn parse_vm_row(v: &Value) -> VmRow {
    VmRow {
        index: count(v, "index"),
        state: text(v, "state"),
        guest_cid: opt_count(v, "guest_cid"),
        serial: opt_text(v, "serial"),
    }
}
