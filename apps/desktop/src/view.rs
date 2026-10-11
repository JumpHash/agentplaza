//! The register as iced draws it: an index on the left, a ruled sheet on the
//! right, one hot signal, and stamps for every state the daemon reports.
//!
//! Every pane is a pure function of the [`Model`], so what the window shows is
//! what the headless reducer decided.

use iced::widget::text::{IntoFragment, Wrapping};
use iced::widget::{
    Column, Container, Row, Text, button, column, container, opaque, row, rule, scrollable, space,
    stack, text, text_input,
};
use iced::{Alignment, Element, Length, Padding};

use crate::api::{FinishOutcome, Run, RunState, Task, TaskArtifact, TaskState, Vm, VmState};
use crate::exit::{ExitKind, exit_kind};
use crate::export::ExportTarget;
use crate::form::{Field, Preset};
use crate::history;
use crate::ledger::{TaskField, artifact_paths};
use crate::mapview::{self, FleetMap};
use crate::model::{
    ARTIFACT_TAIL_LINES, ArtifactState, Health, HistoryStep, Message, Model, Notice, NoticeKind,
    Pane, RosterAgent, SERIAL_TAIL_LINES, duration_label, hub_facts, hub_line, instant_label,
    memory_label, preset_footprint, roster_line, roster_tally, serial_tail, tally_line, task_line,
    task_state_line, task_tally, transcript_span,
};
use crate::telemetryview::{self, ChartKind};
use crate::theme::{self, INK, INK_2, INK_3, MONO, SERIF, Tone, WARN};

/// The guest's command set, as the node's console offers it: the quick buttons
/// beside the serial input, mirroring the web console.
pub const QUICK_COMMANDS: [&str; 6] = ["help", "uname", "cpu", "mem", "uptime", "poweroff"];

/// The whole register: the index and the sheet, with the help sheet over them
/// while it is up.
pub fn view(model: &Model) -> Element<'_, Message> {
    let register = container(row![index(model), sheet(model)])
        .style(theme::paper)
        .width(Length::Fill)
        .height(Length::Fill);
    if !model.help {
        return register.into();
    }
    // The scrim is opaque to the mouse, so nothing behind the sheet can be
    // pressed while it is up; `?` or Escape takes it down.
    stack([register.into(), opaque(help_overlay())]).into()
}

/// The index column: the brand, the fleet, the machine, the map, the runs, the
/// keys, and where the register stands with the daemon.
fn index(model: &Model) -> Container<'_, Message> {
    let (runs, _) = model.fleet_tally();
    let machine_detail = match &model.machine {
        Some(machine) => format!("{} · {}/{}", machine.name, machine.arch, machine.backend),
        None => "not reported yet".to_string(),
    };
    let mut entries: Vec<Element<'_, Message>> = vec![
        mono("AgentPlaza register", 13.0, INK).into(),
        mono("fleet · noded", 10.0, INK_3).into(),
        vertical(10.0),
        mono("Fleet register", 10.5, INK_3).into(),
        rule_heading(),
        index_entry(
            model,
            "0",
            "Fleet",
            format!(
                "{runs} run(s) · {} vm(s) up",
                model.running_vms()
            ),
            Pane::Fleet,
        ),
        index_entry(
            model,
            "1",
            "Roster",
            roster_line(model),
            Pane::Roster,
        ),
        index_entry(model, "2", "Machine", machine_detail, Pane::Machine),
        index_entry(
            model,
            "3",
            "Ledger",
            task_line(model),
            Pane::Ledger,
        ),
        index_entry(
            model,
            "4",
            "Fleet map",
            format!(
                "{runs} lane(s) · {} cell(s)",
                model.runs.iter().map(|run| run.vms.len()).sum::<usize>()
            ),
            Pane::Map,
        ),
        index_entry(
            model,
            "5",
            "Telemetry",
            format!(
                "{} observation(s) · {} lines/s",
                model.history.len(),
                model.fleet_lps()
            ),
            Pane::Telemetry,
        ),
        vertical(12.0),
        mono("Runs", 10.5, INK_3).into(),
        rule_heading(),
    ];

    if model.runs.is_empty() {
        entries.push(
            container(mono("no runs yet", 10.5, INK_3))
                .padding(edge(4.0, 8.0, 4.0, 8.0))
                .into(),
        );
    }
    for run in &model.runs {
        entries.push(index_entry(
            model,
            run.id.as_str(),
            run.state.stamp_label(),
            format!(
                "{}/{} vm · {} vcpu · {} MiB",
                run.live_vms(),
                run.vms.len(),
                run.vcpus,
                run.memory_mib
            ),
            Pane::Run(run.id.clone()),
        ));
    }

    entries.push(vertical(12.0));
    entries.push(
        button(mono("New run", 11.0, INK))
            .on_press(Message::Show(Pane::Spawn))
            .style(theme::primary_button)
            .padding(edge(8.0, 10.0, 8.0, 10.0))
            .width(Length::Fill)
            .into(),
    );
    entries.push(vertical(6.0));
    entries.push(
        button(mono("Help (?)", 11.0, INK_2))
            .on_press(Message::Help(true))
            .style(theme::primary_button)
            .padding(edge(8.0, 10.0, 8.0, 10.0))
            .width(Length::Fill)
            .into(),
    );
    entries.push(vertical(14.0));
    entries.push(rule_heading());
    entries.push(mono(tally_line(model), 10.0, INK_2).into());
    entries.push(
        mono(
            model.committed_line(),
            10.0,
            // The one number in the index that turns critical: the fleet has
            // been asked for more guest memory than the host has.
            if model.overcommitted() {
                theme::CRIT
            } else {
                INK_2
            },
        )
        .into(),
    );
    entries.push(
        mono(
            format!(
                "poll {} ms · polls {} · {}",
                model.refresh_ms,
                model.polls,
                model.health().label()
            ),
            10.0,
            INK_3,
        )
        .into(),
    );
    entries.push(vertical(12.0));
    entries.push(mono("Keys", 10.5, INK_3).into());
    entries.push(rule_heading());
    for (key, what) in KEYS {
        entries.push(
            row![
                mono(key, 10.0, INK).width(38),
                mono(what, 10.0, INK_3),
            ]
            .spacing(4.0)
            .into(),
        );
    }
    entries.push(vertical(12.0));
    entries.push(rule_heading());
    entries.push(
        mono(model.endpoint.url(), 10.0, INK_2)
            .wrapping(Wrapping::None)
            .into(),
    );
    if let Some(age) = model.age_ms() {
        entries.push(mono(format!("last answer {} ago", duration_label(age)), 10.0, INK_3).into());
    }
    if let Some(error) = &model.last_error {
        entries.push(mono(error.as_str(), 10.0, theme::CRIT).into());
    }

    container(scrollable(Column::with_children(entries)).height(Length::Fill))
        .style(theme::index)
        .width(Length::Fixed(280.0))
        .height(Length::Fill)
        .padding(edge(18.0, 14.0, 18.0, 18.0))
}

/// The register's keyboard shortcuts, as the index and the help sheet print
/// them: one table, so the sheet cannot drift from the index.
const KEYS: [(&str, &str); 9] = [
    ("r", "register"),
    ("w", "roster: running + connected, by role"),
    ("l", "ledger: the plaza's tasks"),
    ("m", "machine"),
    ("f", "fleet map"),
    ("t", "telemetry"),
    ("?", "this help"),
    ("esc", "back; closes this sheet"),
    ("↑ ↓", "console history"),
];

/// The help sheet, centred over the register: every key the register binds,
/// and what the sheet is not.
fn help_sheet() -> Element<'static, Message> {
    let mut hints: Vec<Element<'static, Message>> = vec![
        mono("AgentPlaza register", 13.0, INK).into(),
        mono("keys", 10.0, INK_3).into(),
        vertical(4.0),
        rule::horizontal(1.0).style(theme::rule_ink).into(),
    ];
    for (key, what) in KEYS {
        hints.push(
            row![
                mono(key, 11.0, INK).width(Length::Fixed(44.0)),
                mono(what, 11.0, INK_2),
            ]
            .spacing(6.0)
            .into(),
        );
    }
    hints.push(vertical(10.0));
    hints.push(rule_heading());
    hints.push(
        serif(
            "A focused text box captures its own keys, so typing a console line never trips a \
             shortcut. The guest's own commands — help, uname, cpu, mem, uptime, poweroff — are \
             buttons on the run pane, not keys. Escape or ? closes this sheet.",
            12.0,
            INK_2,
        )
        .into(),
    );
    hints.push(vertical(8.0));
    hints.push(
        row![
            button(mono("CLOSE", 10.5, INK))
                .on_press(Message::Help(false))
                .style(theme::primary_button)
                .padding(edge(6.0, 12.0, 6.0, 12.0)),
            mono("the register keeps polling behind this sheet", 10.0, INK_3),
        ]
        .spacing(12.0)
        .align_y(Alignment::Center)
        .into(),
    );

    container(Column::with_children(hints).spacing(4.0))
        .style(theme::sheet)
        .width(Length::Fixed(560.0))
        .padding(edge(20.0, 24.0, 20.0, 24.0))
        .into()
}

/// The help sheet over the register: a scrim across the window with the sheet
/// centred on it.
fn help_overlay() -> Element<'static, Message> {
    container(help_sheet())
        .style(theme::scrim)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

/// One index entry: the number in the margin, the name, and what it holds.
fn index_entry<'a>(
    model: &'a Model,
    number: &'a str,
    name: impl IntoFragment<'a>,
    detail: impl IntoFragment<'a>,
    pane: Pane,
) -> Element<'a, Message> {
    let selected = model.pane == pane;
    button(
        row![
            mono(number, 10.0, INK_3).width(26),
            column![mono(name, 12.0, INK), mono(detail, 9.5, INK_3)].width(Length::Fill),
        ]
        .align_y(Alignment::Center),
    )
    .on_press(Message::Show(pane))
    .style(theme::register_row(selected))
    .padding(edge(6.0, 8.0, 6.0, 4.0))
    .width(Length::Fill)
    .into()
}

/// The sheet: the running head, the notice, and the pane's body.
fn sheet(model: &Model) -> Container<'_, Message> {
    let mut body: Vec<Element<'_, Message>> = vec![running_head(model)];
    if let Some(notice) = &model.notice {
        body.push(notice_line(notice));
    }
    match &model.pane {
        Pane::Fleet => body.push(fleet(model)),
        Pane::Run(id) => body.push(run_pane(model, id)),
        Pane::Spawn => body.push(spawn_pane(model)),
        Pane::Machine => body.push(machine_pane(model)),
        Pane::Map => body.push(map_pane(model)),
        Pane::Roster => body.push(roster_pane(model)),
        Pane::Ledger => body.push(ledger_pane(model)),
        Pane::Telemetry => body.push(telemetry_pane(model)),
    }
    container(Column::with_children(body).spacing(12.0).height(Length::Fill))
        .style(theme::sheet)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(edge(18.0, 22.0, 18.0, 22.0))
}

/// The running head: the folio, the title, the lede, and the connection.
fn running_head(model: &Model) -> Element<'_, Message> {
    let (title, lede) = match &model.pane {
        Pane::Fleet => (
            "Fleet".to_string(),
            "every run the daemon holds".to_string(),
        ),
        Pane::Run(id) => (id.clone(), format!("one run · {}", model.endpoint.url())),
        Pane::Spawn => ("New run".to_string(), "POST /api/v0/runs".to_string()),
        Pane::Machine => (
            "Machine".to_string(),
            match &model.machine {
                Some(machine) => format!("{} · GET /api/v0/machine", machine.model_label()),
                None => "GET /api/v0/machine".to_string(),
            },
        ),
        Pane::Map => (
            "Fleet map".to_string(),
            "the machine's substrate, its runs as lanes, its VMs as cells".to_string(),
        ),
        Pane::Roster => (
            "Roster".to_string(),
            if model.has_hub() {
                "who is running right now · the register's role agents, joined by guest CID to \
                 the hub's connected sessions"
                    .to_string()
            } else {
                "who is running right now · every role agent the register reports".to_string()
            },
        ),
        Pane::Ledger => (
            "Ledger".to_string(),
            "the plaza's tasks · every claim, finish and withdrawal the daemon \
             holds"
                .to_string(),
        ),
        Pane::Telemetry => (
            "Telemetry".to_string(),
            format!(
                "one observation per poll · {} kept of {}",
                model.history.len(),
                model.history.written()
            ),
        ),
    };
    let health = model.health();
    column![
        row![
            mono(format!("f. {}", model.polls), 10.0, INK_3),
            mono(title, 26.0, INK),
            space::horizontal(),
            stamp(health.stamp_label(), tone_for_health(health)),
        ]
        .spacing(10.0)
        .align_y(Alignment::Center),
        serif(lede, 13.0, INK_2),
        rule::horizontal(1.0).style(theme::rule_ink),
    ]
    .spacing(6.0)
    .into()
}

/// The notice line: what the last action did.
fn notice_line(notice: &Notice) -> Element<'_, Message> {
    let tone = match notice.kind {
        NoticeKind::Done => Tone::Ok,
        NoticeKind::Failed => Tone::Crit,
    };
    container(
        row![
            stamp(notice.kind.stamp_label(), tone),
            serif(notice.text.as_str(), 12.5, INK),
            space::horizontal(),
            button(mono("dismiss", 10.0, INK_2))
                .on_press(Message::DismissNotice)
                .style(theme::register_row(false))
                .padding(edge(4.0, 8.0, 4.0, 8.0)),
        ]
        .spacing(10.0)
        .align_y(Alignment::Center),
    )
    .style(theme::notice(tone))
    .width(Length::Fill)
    .padding(edge(8.0, 10.0, 8.0, 10.0))
    .into()
}

/// The fleet register: one ruled row per run.
fn fleet(model: &Model) -> Element<'_, Message> {
    if model.runs.is_empty() {
        let message = if model.health() == Health::Unknown {
            "waiting for the daemon's first answer…"
        } else {
            "no runs: no VM is booted"
        };
        return container(serif(message, 12.5, INK_2))
            .style(theme::boxed)
            .width(Length::Fill)
            .padding(edge(16.0, 14.0, 16.0, 14.0))
            .into();
    }

    let mut rows: Vec<Element<'_, Message>> = vec![
        fleet_head(
            "run", "state", "role", "vms", "vcpus", "memory", "vsock", "created",
        ),
        rule::horizontal(1.0).style(theme::rule_ink).into(),
    ];
    for run in &model.runs {
        rows.push(fleet_row(model, run));
    }
    Column::with_children(rows).spacing(3.0).into()
}

/// The fleet register's column headings.
fn fleet_head(
    run: &'static str,
    state: &'static str,
    role: &'static str,
    vms: &'static str,
    vcpus: &'static str,
    memory: &'static str,
    vsock: &'static str,
    created: &'static str,
) -> Element<'static, Message> {
    Row::with_children(vec![
        mono(run, 10.0, INK_3).width(Length::FillPortion(2)).into(),
        mono(state, 10.0, INK_3)
            .width(Length::FillPortion(2))
            .into(),
        mono(role, 10.0, INK_3).width(Length::FillPortion(3)).into(),
        mono(vms, 10.0, INK_3).width(Length::FillPortion(3)).into(),
        mono(vcpus, 10.0, INK_3)
            .width(Length::FillPortion(2))
            .into(),
        mono(memory, 10.0, INK_3)
            .width(Length::FillPortion(2))
            .into(),
        mono(vsock, 10.0, INK_3)
            .width(Length::FillPortion(2))
            .into(),
        mono(created, 10.0, INK_3)
            .width(Length::FillPortion(3))
            .into(),
    ])
    .spacing(8.0)
    .into()
}

/// A run's roles as the register stamps them: the role itself for a run
/// booted as one role, the count for a run carrying several (the daemon's role
/// fleet is one run of twenty-one), and a dash for a run whose VMs carry none
/// — the register's own way of writing nothing.
fn role_cell<'a>(run: &'a Run) -> Element<'a, Message> {
    let roles = run.roles();
    match roles.as_slice() {
        [] => mono("—", 10.5, INK_3)
            .width(Length::FillPortion(3))
            .into(),
        [role] => stamp(role.to_uppercase(), Tone::Neutral)
            .width(Length::FillPortion(3))
            .into(),
        roles => stamp(format!("{} ROLES", roles.len()), Tone::Neutral)
            .width(Length::FillPortion(3))
            .into(),
    }
}

/// One run's row, which opens the run when pressed.
fn fleet_row<'a>(model: &'a Model, run: &'a Run) -> Element<'a, Message> {
    let selected = model.pane == Pane::Run(run.id.clone());
    let vms = format!("{}/{} up", run.live_vms(), run.vms.len());
    let vsock = match run.base_cid() {
        Some(cid) => format!("{cid}+"),
        None => "—".to_string(),
    };
    // A fleet run stays `running` for days: its age is read against the
    // register's clock, and its creation is written as the day it was when
    // that is not this day.
    let created = format!(
        "{} · {}",
        instant_label(run.created_at, model.run_reference_ms(run)),
        duration_label(model.run_elapsed_ms(run))
    );

    button(
        row![
            mono(run.id.as_str(), 12.0, INK).width(Length::FillPortion(2)),
            stamp(run.state.stamp_label(), tone_for_run(run.state)).width(Length::FillPortion(2)),
            role_cell(run),
            mono(vms, 10.5, INK_2).width(Length::FillPortion(3)),
            mono(run.vcpus.to_string(), 10.5, INK_2).width(Length::FillPortion(2)),
            mono(format!("{} MiB", run.memory_mib), 10.5, INK_2)
                .width(Length::FillPortion(2)),
            mono(vsock, 10.5, INK_2).width(Length::FillPortion(2)),
            mono(created, 10.5, INK_2).width(Length::FillPortion(3)),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center),
    )
    .on_press(Message::Show(Pane::Run(run.id.clone())))
    .style(theme::register_row(selected))
    .padding(edge(6.0, 6.0, 6.0, 6.0))
    .width(Length::Fill)
    .into()
}

/// The machine: the node's own model, as a ruled sheet of the daemon's
/// columns, with its uptime ticking and its vsock transport named.
fn machine_pane(model: &Model) -> Element<'_, Message> {
    let Some(machine) = model.machine.as_ref() else {
        let message = if model.health() == Health::Down {
            "the machine endpoint did not answer; the index carries the daemon's own words"
        } else {
            "asking the daemon for its machine…"
        };
        return container(serif(message, 12.5, INK_2))
            .style(theme::boxed)
            .width(Length::Fill)
            .padding(edge(16.0, 14.0, 16.0, 14.0))
            .into();
    };

    let uptime = model.machine_uptime_ms().unwrap_or(0);
    let mut rows: Vec<Element<'_, Message>> = vec![
        column![
            mono(machine.model_label(), 15.0, INK),
            serif(
                "the node's own model: the substrate every run of this daemon is booted on",
                12.0,
                INK_2,
            ),
        ]
        .spacing(4.0)
        .into(),
        rule::horizontal(1.0).style(theme::rule_ink).into(),
        lead(vec![
            ("uptime", duration_label(uptime)),
            ("vsock_mode", machine.vsock_mode.clone()),
            ("class", machine.class.clone()),
            ("irqchip", machine.irqchip.clone()),
        ]),
        vertical(6.0),
        rule::horizontal(1.0).style(theme::rule).into(),
        row![
            mono("column", 10.0, INK_3).width(Length::Fixed(150.0)),
            mono("value", 10.0, INK_3).width(Length::Fixed(220.0)),
            mono("what the daemon reports", 10.0, INK_3),
        ]
        .spacing(12.0)
        .into(),
        rule_heading(),
    ];
    for (column, value, note) in [
        ("name", machine.name.clone(), "the host's hostname"),
        ("arch", machine.arch.clone(), "x86_64 / aarch64"),
        (
            "backend",
            machine.backend.clone(),
            "the VMM backend its VMs are booted with",
        ),
        (
            "irqchip",
            machine.irqchip.clone(),
            "the interrupt controller in use",
        ),
        (
            "class",
            machine.class.clone(),
            "server / apple / pi4 / pi5",
        ),
        ("cpus", machine.cpus.to_string(), "the host's cores"),
        (
            "memory_mib",
            format!("{} MiB", machine.memory_mib),
            "the host's total memory",
        ),
        (
            "vsock_mode",
            machine.vsock_mode.clone(),
            "the transport every VM's vsock is wired through",
        ),
        (
            "started_at",
            instant_label(machine.started_at_ms, model.now_ms),
            "when the daemon started",
        ),
        (
            "uptime",
            duration_label(uptime),
            "time since the daemon started, ticking with the poll",
        ),
    ] {
        rows.push(machine_row(column, value, note));
    }
    Column::with_children(rows).spacing(5.0).into()
}

/// One machine column: its name, the daemon's value, and what it holds.
fn machine_row<'a>(
    column: &'static str,
    value: String,
    note: &'static str,
) -> Element<'a, Message> {
    row![
        mono(column, 11.0, INK).width(Length::Fixed(150.0)),
        mono(value, 11.5, INK_2).width(Length::Fixed(220.0)),
        mono(note, 9.5, INK_3),
    ]
    .spacing(12.0)
    .align_y(Alignment::Center)
    .into()
}

/// The fleet map: the plate itself, and the legend that names what it draws.
fn map_pane(model: &Model) -> Element<'_, Message> {
    let selected = model
        .selection
        .as_ref()
        .map(|selection| (selection.run.as_str(), selection.vm));
    let plate = container(mapview::widget(FleetMap {
        runs: &model.runs,
        activity: &model.activity,
        machine: model.machine.as_ref(),
        selected,
    }))
    .style(theme::boxed)
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(edge(4.0, 4.0, 4.0, 4.0));

    let cells: usize = model.runs.iter().map(|run| run.vms.len()).sum();
    let status = if model.runs.is_empty() {
        "the plate is blank: no run has been booted on this node".to_string()
    } else {
        format!(
            "{} lane(s) · {cells} cell(s) · press a cell to open its run with that vm selected",
            model.runs.len()
        )
    };

    Column::with_children(vec![
        serif(
            "The machine is the bounded substrate. A run is a lane, a VM is a cell, placed \
             by index. Up is a filled dot, exited a square, killed a diamond; the line through \
             a mark is the VM's vsock device, and the bar beside it is its serial rate in \
             lines per second, measured between polls.",
            12.0,
            INK_2,
        )
        .into(),
        plate.into(),
        mono(status, 10.5, INK_3).into(),
        mono(map_status(model), 10.5, INK_2).into(),
    ])
    .spacing(10.0)
    .height(Length::Fill)
    .into()
}

/// What the plate is drawing: the substrate's model, and the pressed cell's
/// real state.
fn map_status(model: &Model) -> String {
    let mut text = match &model.machine {
        Some(machine) => format!("substrate · {}", machine.model_label()),
        None => "substrate · the machine has not been reported yet".to_string(),
    };
    if let Some(selection) = &model.selection {
        let vm = model
            .runs
            .iter()
            .find(|run| run.id == selection.run)
            .and_then(|run| run.vms.iter().find(|vm| vm.id == selection.vm));
        text.push_str(&format!(" · selected {} vm {}", selection.run, selection.vm));
        if let Some(vm) = vm {
            text.push_str(&format!(" · {}", vm.state.label()));
            if let Some(cid) = vm.guest_cid {
                text.push_str(&format!(" · cid {cid}"));
            }
            if let Some(serial) = vm.serial.as_deref() {
                text.push_str(&format!(" · {} serial bytes", serial.len()));
            }
        }
    }
    text
}

/// The roster: who is running right now — the register's report — and, when a
/// hub url is configured, who is connected to it.
///
/// The two halves are joined by guest CID and never merged, so an agent one of
/// them reports and the other does not is drawn as exactly that. With no hub
/// configured the sheet draws the register's half alone, as it did before
/// there was a hub, and the hub header says why.
fn roster_pane(model: &Model) -> Element<'_, Message> {
    let groups = model.roster();
    let live = model.roster_live();
    let hub_known = model.hub_status.is_some();

    let mut body: Vec<Element<'_, Message>> = vec![
        serif(
            "One row per role agent: the register's side (its run, VM, guest CID, state and \
             serial rate) and the hub's side (the session it holds — when it connected, its last \
             heartbeat and the calls it has served). The halves are joined by guest CID and never \
             merged, so an agent only one of the two reports is drawn as exactly that. Grouped by \
             role, in the order the roles appear.",
            12.0,
            INK_2,
        )
        .into(),
        hub_header(model),
    ];

    if groups.is_empty() {
        let message = if model.health() == Health::Unknown && !hub_known {
            "waiting for the daemon's first answer…"
        } else if hub_known {
            "no role agent is on either surface: the daemon reports a role on a VM's row only \
             when it booted that VM as one (noded --agents <manifest>), and the hub reports no \
             connected agent. Nothing here is a role fleet yet."
        } else {
            "no role agent is in the register: the daemon reports a role on a VM's row only when \
             it booted that VM as one (noded --agents <manifest>). Nothing here is a role fleet \
             yet."
        };
        body.push(
            container(serif(message, 12.5, INK_2))
                .style(theme::boxed)
                .width(Length::Fill)
                .padding(edge(16.0, 14.0, 16.0, 14.0))
                .into(),
        );
        return scrollable(Column::with_children(body).spacing(10.0))
            .height(Length::Fill)
            .into();
    }

    body.push(
        mono(
            roster_tally(&groups),
            11.5,
            if live > 0 { INK } else { INK_2 },
        )
        .into(),
    );
    body.push(rule::horizontal(1.0).style(theme::rule_ink).into());

    for group in &groups {
        let mut counts = format!("{} agent(s) · {} up", group.agents.len(), group.live());
        if hub_known {
            counts.push_str(&format!(" · {} connected", group.connected()));
        }
        counts.push_str(&format!(" · {:.1} line(s)/s", group.lps()));
        body.push(
            row![
                stamp(group.role.to_uppercase(), Tone::Neutral),
                mono(group.role.clone(), 12.0, INK),
                space::horizontal(),
                mono(counts, 10.5, INK_3),
            ]
            .spacing(8.0)
            .align_y(Alignment::Center)
            .into(),
        );
        body.push(roster_heads(hub_known));
        body.push(rule_heading());
        for agent in &group.agents {
            body.push(roster_row(agent, hub_known, model.now_ms));
        }
        body.push(vertical(8.0));
    }
    if live == 0 {
        body.push(
            serif(
                "No role agent is up right now: the register reports every role below, and every \
                 one of their VMs has stopped.",
                12.0,
                INK_2,
            )
            .into(),
        );
    }

    // A role per the codex is twenty-one in this fleet, and each group is a
    // heading, a table and its agents: the sheet scrolls rather than showing
    // the first few roles and silently hiding the rest.
    scrollable(Column::with_children(body).spacing(6.0))
        .height(Length::Fill)
        .into()
}

/// The roster's hub header: what the status surface says — its host facts and
/// its live counts — or why there is nothing to say.
///
/// Always drawn, because "who is connected" has three honest answers and the
/// operator should never have to guess which one the sheet is giving: no hub
/// was configured, the hub answered, or the hub could not be read.
fn hub_header(model: &Model) -> Element<'_, Message> {
    let line: Element<'_, Message> = match (&model.hub, &model.hub_status) {
        (None, _) => mono("hub unknown (no --hub-url)", 11.5, INK_3).into(),
        (Some(_), Some(status)) => column![
            mono(hub_line(status), 11.5, INK),
            mono(hub_facts(status), 10.5, INK_3),
        ]
        .spacing(2.0)
        .into(),
        (Some(_), None) => match &model.hub_error {
            Some(error) => mono(format!("hub unreachable ({error})"), 11.5, WARN).into(),
            None => mono("hub: waiting for its first answer…", 11.5, INK_3).into(),
        },
    };
    container(line)
        .style(theme::boxed)
        .width(Length::Fill)
        .padding(edge(14.0, 10.0, 14.0, 10.0))
        .into()
}

/// The roster's column widths: one place for the heads and the cells, so the
/// table cannot drift from itself, and each wide enough for the widest value
/// it prints — a cell the text does not fit is a cell that wraps.
const COL_CID: f32 = 72.0;
const COL_RUN: f32 = 44.0;
const COL_VM: f32 = 44.0;
const COL_STATE: f32 = 88.0;
const COL_RATE: f32 = 56.0;
const COL_CONNECTED: f32 = 88.0;
const COL_BEAT: f32 = 84.0;
const COL_CALLS: f32 = 84.0;

/// The register's half of a row, summed: what the missing-half label has to
/// occupy to keep the hub's half in its columns.
const ROSTER_REGISTER_HALF: f32 =
    COL_RUN + 8.0 + COL_VM + 8.0 + COL_STATE + 8.0 + COL_RATE;

/// The roster's column heads, matched to the cells below them: the join key,
/// the register's half, and — when the hub was read — the hub's half.
fn roster_heads(hub_known: bool) -> Element<'static, Message> {
    let mut cells: Vec<Element<'static, Message>> = vec![
        mono("guest cid", 10.0, INK_3).width(Length::Fixed(COL_CID)).into(),
        mono("run", 10.0, INK_3).width(Length::Fixed(COL_RUN)).into(),
        mono("vm", 10.0, INK_3).width(Length::Fixed(COL_VM)).into(),
        mono("state", 10.0, INK_3).width(Length::Fixed(COL_STATE)).into(),
        mono("lines/s", 10.0, INK_3).width(Length::Fixed(COL_RATE)).into(),
    ];
    if hub_known {
        cells.push(mono("connected", 10.0, INK_3).width(Length::Fixed(COL_CONNECTED)).into());
        cells.push(mono("heartbeat", 10.0, INK_3).width(Length::Fixed(COL_BEAT)).into());
        cells.push(mono("calls", 10.0, INK_3).width(Length::Fixed(COL_CALLS)).into());
        cells.push(mono("role (hub)", 10.0, INK_3).into());
    }
    Row::with_children(cells).spacing(8.0).into()
}

/// One agent's row: the CID the halves were joined on, then the register's
/// side and the hub's side, each drawn from what that surface actually
/// reports.
///
/// Pressing it opens its run with its VM selected — the same target the map's
/// cells and the console use — when the register reports one. A row only the
/// hub knows has no run to open, so it is not a button: a press that goes
/// nowhere is worse than no press at all.
fn roster_row(agent: &RosterAgent, hub_known: bool, now_ms: u64) -> Element<'static, Message> {
    let cid = match agent.cid() {
        Some(cid) => format!("cid {cid}"),
        None => "no vsock".to_string(),
    };
    let mut cells: Vec<Element<'static, Message>> = vec![
        mono(cid, 10.5, INK_2).width(Length::Fixed(COL_CID)).into(),
        register_half(agent),
    ];
    if hub_known {
        cells.push(hub_half(agent, now_ms));
    }
    let row = Row::with_children(cells).spacing(8.0).align_y(Alignment::Center);
    match &agent.registered {
        Some(registered) => button(row)
            .on_press(Message::Select {
                run: registered.run.clone(),
                vm: registered.vm,
            })
            .style(theme::register_row(false))
            .padding(edge(4.0, 6.0, 4.0, 6.0))
            .width(Length::Fill)
            .into(),
        None => container(row)
            .padding(edge(4.0, 6.0, 4.0, 6.0))
            .width(Length::Fill)
            .into(),
    }
}

/// The register's half of an agent's row: which run and VM its guest is, the
/// state it is in and the serial rate the activity feed last measured — or the
/// honest word that the register does not report the agent at all.
fn register_half(agent: &RosterAgent) -> Element<'static, Message> {
    let Some(registered) = &agent.registered else {
        return mono("connected, not registered", 10.5, WARN)
            .width(Length::Fixed(ROSTER_REGISTER_HALF))
            .into();
    };
    let rate = if registered.lps > 0.02 {
        format!("{:.1}", registered.lps)
    } else {
        "—".to_string()
    };
    Row::with_children(vec![
        mono(registered.run.clone(), 11.0, INK)
            .width(Length::Fixed(COL_RUN))
            .into(),
        mono(format!("vm {:02}", registered.vm), 10.5, INK_2)
            .width(Length::Fixed(COL_VM))
            .into(),
        stamp(registered.state.stamp_label(), tone_for_vm(registered.state))
            .width(Length::Fixed(COL_STATE))
            .into(),
        mono(rate, 10.5, INK_2)
            .width(Length::Fixed(COL_RATE))
            .into(),
    ])
    .spacing(8.0)
    .align_y(Alignment::Center)
    .into()
}

/// The hub's half of an agent's row: when its session connected, how long ago
/// it last beat and how many calls it has served — or the honest word that the
/// hub reports nobody on its CID.
///
/// The words for a missing half are toned by what they mean: a VM that is up
/// with nobody connected to it is worth a second look, while one that has
/// stopped has simply left.
fn hub_half(agent: &RosterAgent, now_ms: u64) -> Element<'static, Message> {
    let Some(connected) = &agent.connected else {
        let tone = if agent.is_live() { WARN } else { INK_3 };
        return mono("registered, not connected", 10.5, tone).into();
    };
    let role_tone = if agent.role_mismatch() { WARN } else { INK_2 };
    Row::with_children(vec![
        stamp("CONNECTED", Tone::Ok)
            .width(Length::Fixed(COL_CONNECTED))
            .into(),
        mono(instant_label(connected.connected_at_ms, now_ms), 10.5, INK_2)
            .width(Length::Fixed(COL_BEAT))
            .into(),
        mono(
            format!("{} ago", duration_label(connected.heartbeat_age_ms(now_ms))),
            10.5,
            INK_2,
        )
        .width(Length::Fixed(COL_BEAT))
        .into(),
        mono(format!("{} call(s)", connected.calls), 10.5, INK_2)
            .width(Length::Fixed(COL_CALLS))
            .into(),
        mono(format!("role {}", connected.role), 10.5, role_tone).into(),
    ])
    .spacing(8.0)
    .align_y(Alignment::Center)
    .into()
}

/// The ledger: the plaza's tasks as an account book — one ruled row per task,
/// the open task's detail, and the four moves the daemon accepts.
///
/// The rows come from one read (`GET /api/v0/tasks`, refreshed on the poll
/// while this sheet is up), and the task events patch them in place between
/// the polls: an event for a task the ledger has not read asks for the ledger
/// rather than inventing a row from a frame that carries an id, a state and an
/// assignee and nothing else.
fn ledger_pane(model: &Model) -> Element<'_, Message> {
    let mut body: Vec<Element<'_, Message>> = vec![
        serif(
            "One row per task: what it is for, the role it was filed for, its budget, the state \
             it is in, who holds it and the guest CID it is held by, and when it was filed and \
             last moved. A task is claimed by a live agent, finished as done or failed with a \
             note, or withdrawn before it ends; a task that has ended is final. Refusals — the \
             daemon's own, or one of these guards — land in the notice line above.",
            12.0,
            INK_2,
        )
        .into(),
    ];

    if model.tasks.is_empty() {
        let message = if !model.ledger_read && model.health() == Health::Unknown {
            "waiting for the daemon's first answer…"
        } else {
            "no task is on the ledger: nothing has been filed yet"
        };
        body.push(
            container(serif(message, 12.5, INK_2))
                .style(theme::boxed)
                .width(Length::Fill)
                .padding(edge(16.0, 14.0, 16.0, 14.0))
                .into(),
        );
    } else {
        body.push(mono(task_tally(&model.tasks), 11.5, INK).into());
        body.push(task_heads());
        body.push(rule_heading());
        for task in &model.tasks {
            body.push(task_row(model, task));
        }
    }

    match model.selected_task() {
        Some(task) => body.push(task_detail(model, task)),
        None if !model.tasks.is_empty() => body.push(
            container(serif(
                "Press a task's row to open its detail and the moves the daemon accepts on it.",
                11.5,
                INK_3,
            ))
            .style(theme::boxed)
            .width(Length::Fill)
            .padding(edge(12.0, 12.0, 12.0, 12.0))
            .into(),
        ),
        None => {}
    }

    body.push(task_create_form(model));
    // The ledger's rows are columns of the sheet: a scrollable measures its
    // content against the viewport, so the content is told to take it — which
    // is what lets the goal's own column fill what the stamped columns leave.
    Column::with_children(body).spacing(8.0).width(Length::Fill).into()
}

/// The ledger's column widths: one place for the heads and the cells, each
/// wide enough for the widest value it prints — a cell the text does not fit
/// is a cell that wraps.
const TASK_COL_ID: f32 = 40.0;
const TASK_COL_GOAL: f32 = 180.0;
const TASK_COL_ROLE: f32 = 100.0;
const TASK_COL_BUDGET: f32 = 62.0;
const TASK_COL_STATE: f32 = 98.0;
const TASK_COL_ASSIGNEE: f32 = 100.0;
const TASK_COL_CID: f32 = 74.0;
const TASK_COL_STAMP: f32 = 92.0;

/// How much space is between the ledger's columns, heads and cells both: one
/// constant, so the table's own width is a sum rather than a guess.
const COL_SPACING: f32 = 6.0;

/// The height of the box an artifact's text is read in: bounded, so a long
/// file scrolls inside it rather than stretching the ledger's detail down the
/// sheet.
const ARTIFACT_VIEW_HEIGHT: f32 = 220.0;

/// The ledger's column heads, matched to the cells below them: the task, what
/// it is for, and everything the daemon reports about how it stands.
fn task_heads() -> Element<'static, Message> {
    Row::with_children(vec![
        mono("id", 10.0, INK_3).width(Length::Fixed(TASK_COL_ID)).into(),
        mono("goal", 10.0, INK_3).width(Length::Fixed(TASK_COL_GOAL)).into(),
        mono("role", 10.0, INK_3).width(Length::Fixed(TASK_COL_ROLE)).into(),
        mono("budget", 10.0, INK_3).width(Length::Fixed(TASK_COL_BUDGET)).into(),
        mono("state", 10.0, INK_3).width(Length::Fixed(TASK_COL_STATE)).into(),
        mono("assignee", 10.0, INK_3).width(Length::Fixed(TASK_COL_ASSIGNEE)).into(),
        mono("agent cid", 10.0, INK_3).width(Length::Fixed(TASK_COL_CID)).into(),
        mono("created", 10.0, INK_3).width(Length::Fixed(TASK_COL_STAMP)).into(),
        mono("updated", 10.0, INK_3).width(Length::Fixed(TASK_COL_STAMP)).into(),
    ])
    .spacing(COL_SPACING)
    .into()
}

/// One task's row, which opens its detail when pressed.
fn task_row<'a>(model: &'a Model, task: &'a Task) -> Element<'a, Message> {
    let selected = model.task_selected.as_deref() == Some(task.id.as_str());
    let cells = Row::with_children(vec![
        mono(task.id.as_str(), 11.0, INK).width(Length::Fixed(TASK_COL_ID)).into(),
        mono(task.goal.as_str(), 11.0, INK)
            .width(Length::Fixed(TASK_COL_GOAL))
            .wrapping(Wrapping::None)
            .into(),
        mono(column_or_dash(task.role.as_deref()), 10.5, INK_2)
            .width(Length::Fixed(TASK_COL_ROLE))
            .wrapping(Wrapping::None)
            .into(),
        mono(task.budget.to_string(), 10.5, INK_2)
            .width(Length::Fixed(TASK_COL_BUDGET))
            .into(),
        stamp(task.state.stamp_label(), tone_for_task(task.state))
            .width(Length::Fixed(TASK_COL_STATE))
            .into(),
        mono(column_or_dash(task.assignee_id.as_deref()), 10.5, INK_2)
            .width(Length::Fixed(TASK_COL_ASSIGNEE))
            .wrapping(Wrapping::None)
            .into(),
        mono(agent_cid_label(task), 10.5, INK_2)
            .width(Length::Fixed(TASK_COL_CID))
            .into(),
        // A task filed days ago reads as its day rather than as a time of day
        // that would read as today's, exactly as a run's creation does.
        mono(instant_label(task.created_at, model.now_ms), 10.5, INK_2)
            .width(Length::Fixed(TASK_COL_STAMP))
            .into(),
        mono(instant_label(task.updated_at, model.now_ms), 10.5, INK_2)
            .width(Length::Fixed(TASK_COL_STAMP))
            .into(),
    ])
    .spacing(COL_SPACING)
    .align_y(Alignment::Center);

    button(cells)
        .on_press(Message::SelectTask(task.id.clone()))
        .style(theme::register_row(selected))
        .padding(edge(4.0, 6.0, 4.0, 6.0))
        .width(Length::Fill)
        .into()
}

/// The open task's detail: every fact the daemon reports about the row, the
/// text the task carries, and the moves the daemon would accept on it.
fn task_detail<'a>(model: &'a Model, task: &'a Task) -> Element<'a, Message> {
    let mut body: Vec<Element<'a, Message>> = vec![
        row![
            mono(format!("task {}", task.id), 15.0, INK),
            stamp(task.state.stamp_label(), tone_for_task(task.state)),
            space::horizontal(),
            button(mono("REFRESH", 10.5, INK))
                .on_press(Message::TaskRefresh)
                .style(theme::primary_button)
                .padding(edge(6.0, 10.0, 6.0, 10.0)),
        ]
        .spacing(10.0)
        .align_y(Alignment::Center)
        .into(),
        serif(task.goal.as_str(), 13.0, INK).into(),
        serif(
            match task.acceptance.as_deref() {
                Some(acceptance) => acceptance.to_string(),
                None => "no acceptance criteria were given for this task".to_string(),
            },
            12.0,
            match task.acceptance.get() {
                Some(_) => INK_2,
                None => INK_3,
            },
        )
        .into(),
        rule::horizontal(1.0).style(theme::rule).into(),
        lead(vec![
            ("state", task_state_line(task)),
            ("budget", format!("{} credits", task.budget)),
            ("role", column_or_dash(task.role.as_deref()).to_string()),
            ("assignee", column_or_dash(task.assignee_id.as_deref()).to_string()),
            ("agent cid", agent_cid_label(task)),
            (
                "verifier",
                match task.verifier_id.as_deref() {
                    Some(verifier) => verifier.to_string(),
                    // v0's ledger has no verification step, and says so rather
                    // than drawing a name nobody recorded.
                    None => "none (v0 records no verifier)".to_string(),
                },
            ),
            ("created", instant_label(task.created_at, model.now_ms)),
            (
                "updated",
                format!(
                    "{} · {} ago",
                    instant_label(task.updated_at, model.now_ms),
                    duration_label(model.now_ms.saturating_sub(task.updated_at))
                ),
            ),
        ]),
        serif(
            match task.note.as_deref() {
                Some(note) => format!("The agent's note: {note}"),
                None => "No note was left on this task.".to_string(),
            },
            if task.note.get().is_some() { 12.5 } else { 11.5 },
            if task.note.get().is_some() { INK } else { INK_3 },
        )
        .into(),
    ];

    if let Some(section) = task_artifact_section(model, task) {
        body.push(section);
    }

    if task.state.is_finished() {
        body.push(
            container(serif(
                format!(
                    "The task has ended ({}) and its state is final: the daemon accepts no \
                     further claim, finish or withdrawal on it.",
                    task_state_line(task)
                ),
                11.5,
                INK_2,
            ))
            .style(theme::boxed)
            .width(Length::Fill)
            .padding(edge(12.0, 10.0, 12.0, 10.0))
            .into(),
        );
        return Column::with_children(body).spacing(8.0).into();
    }

    if task.state.is_claimable() {
        body.push(task_claim_row(model));
    }
    if task.state.is_finishable() {
        body.push(task_finish_row(model));
    }
    body.push(task_withdraw_row(model, task));
    Column::with_children(body).spacing(8.0).into()
}

/// The artifact section: the files the open task's finish note names, each
/// readable through the daemon and the task's own agent, and the one that was
/// read.
///
/// The paths come out of the note the dispatcher left ([`artifact_paths`]).
/// Each row drives the task agent's `read_file` through the daemon
/// (`POST /api/v0/tasks/{id}/artifact`), and what comes back is the agent's
/// own file, drawn as ruled text and never interpreted: it is the guest's
/// bytes. A read is in one of three honest states when it is not in flight —
/// the agent read it, the daemon refused it in its own words, or the daemon
/// was never told where the hub's admin surface is.
///
/// A note that names no artifact (a read-only run's summary line, or a task
/// nobody has finished) gets no section: there is nothing to read, and saying
/// so would only be noise.
fn task_artifact_section<'a>(model: &'a Model, task: &'a Task) -> Option<Element<'a, Message>> {
    let paths = match task.note.as_deref() {
        Some(note) => artifact_paths(note),
        None => Vec::new(),
    };
    // The read belongs to the task it was asked of: a read for another task
    // is not drawn under this one.
    let state = model
        .artifact
        .as_ref()
        .filter(|state| state.task() == task.id);
    if paths.is_empty() && state.is_none() {
        return None;
    }

    let mut body: Vec<Element<'a, Message>> = vec![
        row![
            mono("Artifacts", 10.5, INK_3),
            space::horizontal(),
            mono(
                format!("POST /api/v0/tasks/{}/artifact", task.id),
                10.0,
                INK_3,
            ),
        ]
        .align_y(Alignment::Center)
        .into(),
        rule_heading(),
    ];
    for path in &paths {
        let selected = state.is_some_and(|state| state.path() == path.as_str());
        body.push(artifact_row(path.clone(), selected));
    }
    match state {
        Some(ArtifactState::Loading { path, .. }) => body.push(
            serif(
                format!("Reading {path} through task {}'s agent…", task.id),
                11.5,
                INK_3,
            )
            .into(),
        ),
        Some(ArtifactState::Read { artifact, .. }) => body.extend(artifact_read(artifact)),
        Some(ArtifactState::Refused { path, error, .. }) => body.push(
            serif(format!("{path} was not read: {error}"), 11.5, theme::CRIT).into(),
        ),
        Some(ArtifactState::NotConfigured { path, message, .. }) => body.push(
            serif(
                format!(
                    "{path} was not read: the daemon was not told where the hub's admin \
                     surface is — {message}"
                ),
                11.5,
                INK_2,
            )
            .into(),
        ),
        None => {}
    }
    Some(Column::with_children(body).spacing(6.0).into())
}

/// One artifact path as a selectable row: pressing it reads that file through
/// the task's agent. The row the pane is showing is marked, so a re-read of the
/// same path is a knowing choice rather than a guess.
fn artifact_row<'a>(path: String, selected: bool) -> Element<'a, Message> {
    button(
        row![
            mono("read", 10.0, INK_3).width(Length::Fixed(34.0)),
            mono(path.clone(), 11.5, INK).width(Length::Fill),
            mono(
                if selected {
                    "shown below"
                } else {
                    "through the task's agent"
                },
                9.5,
                INK_3,
            ),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center),
    )
    .on_press(Message::TaskArtifactRead(path))
    .style(theme::register_row(selected))
    .padding(edge(4.0, 6.0, 4.0, 6.0))
    .width(Length::Fill)
    .into()
}

/// One artifact the agent read: a heading naming the file, its span and the
/// agent that read it, and the text itself — ruled and bounded, every line in
/// the margin with the byte it starts at, exactly as the transcript is drawn.
/// The bytes are the guest's, so they are displayed and never interpreted.
fn artifact_read<'a>(artifact: &'a TaskArtifact) -> Vec<Element<'a, Message>> {
    let (skipped, rows) = serial_tail(&artifact.content, ARTIFACT_TAIL_LINES);
    let heading = match transcript_span(&artifact.content, &rows) {
        Some((from, to)) => format!(
            "{} · {} line(s) · byte {from}–{to} · {} bytes · read by agent cid {}",
            artifact.path,
            rows.len(),
            artifact.size,
            artifact.agent_cid
        ),
        // A file the agent read as empty has no span to name, and says so
        // rather than naming a byte nobody read.
        None => format!(
            "{} · no bytes · read by agent cid {}",
            artifact.path, artifact.agent_cid
        ),
    };
    let mut lines: Vec<Element<'a, Message>> = vec![mono(heading, 10.0, INK_3).into()];
    if artifact.truncated {
        lines.push(
            mono(
                "the agent's own read cap ended this file: what is shown is not all of it",
                10.0,
                WARN,
            )
            .into(),
        );
    }
    if skipped > 0 {
        lines.push(mono(format!("… {skipped} earlier line(s) not shown"), 10.0, INK_3).into());
    }
    for row in &rows {
        lines.push(
            row![
                mono(row.offset.to_string(), 9.5, INK_3)
                    .width(Length::Fixed(58.0))
                    .align_x(iced::alignment::Horizontal::Right),
                mono(row.text, 11.0, INK),
            ]
            .spacing(8.0)
            .into(),
        );
    }
    vec![container(
        scrollable(Column::with_children(lines).spacing(1.0).width(Length::Fill))
            .height(Length::Fixed(ARTIFACT_VIEW_HEIGHT)),
    )
    .style(theme::boxed)
    .width(Length::Fill)
    .padding(edge(10.0, 10.0, 10.0, 10.0))
    .into()]
}

/// The claim row: the agent's guest CID, the button that claims as it, and —
/// as one-press buttons — the CIDs the register shows a live agent on.
///
/// The buttons fill the box rather than claiming on the spot, the same way the
/// spawn form's quick boots fill the form: the CID is visible, and adjustable,
/// before anything is sent. They are the CIDs a claim would be *accepted*
/// from — the daemon accepts one only from an agent its register shows
/// running — so this turns the daemon's own rule into something the operator
/// can point at instead of typing a number the daemon will refuse.
fn task_claim_row(model: &Model) -> Element<'_, Message> {
    let mut body: Vec<Element<'_, Message>> = vec![
        row![
            text_input("the agent's guest CID", model.claim_cid.as_str())
                .on_input(Message::ClaimCidEdit)
                .on_submit(Message::TaskClaim)
                .font(MONO)
                .size(12.0)
                .padding(edge(7.0, 9.0, 7.0, 9.0))
                .width(Length::Fixed(200.0))
                .style(theme::input),
            button(mono("CLAIM", 10.5, INK))
                .on_press(Message::TaskClaim)
                .style(theme::primary_button)
                .padding(edge(6.0, 12.0, 6.0, 12.0)),
            mono("binds this open task to the agent connected as that CID", 10.0, INK_3),
        ]
        .spacing(10.0)
        .align_y(Alignment::Center)
        .into(),
    ];

    let agents = model.live_agents();
    if agents.is_empty() {
        body.push(serif(
            "No live agent is in the register: a claim is accepted only from an agent that is up, \
             so there is no CID to claim as yet.",
            11.5,
            INK_3,
        )
        .into());
        return Column::with_children(body).spacing(6.0).into();
    }
    let mut buttons: Vec<Element<'_, Message>> = Vec::with_capacity(agents.len());
    for (cid, role) in agents {
        buttons.push(
            button(mono(format!("cid {cid} · {role}"), 10.0, INK))
                .on_press(Message::ClaimCidEdit(cid.to_string()))
                .style(theme::primary_button)
                .padding(edge(4.0, 9.0, 4.0, 9.0))
                .into(),
        );
    }
    body.push(
        row![
            mono("live agents", 10.0, INK_3).width(Length::Fixed(88.0)),
            Row::with_children(buttons).spacing(6.0),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center)
        .into(),
    );
    Column::with_children(body).spacing(6.0).into()
}

/// The finish row: the note the claiming agent leaves, and the two outcomes it
/// can report.
fn task_finish_row(model: &Model) -> Element<'_, Message> {
    Column::with_children(vec![
        row![
            mono("note", 10.0, INK_3).width(Length::Fixed(88.0)),
            text_input("what the agent has to say (optional)", model.finish_note.as_str())
                .on_input(Message::FinishNoteEdit)
                .font(MONO)
                .size(12.0)
                .padding(edge(7.0, 9.0, 7.0, 9.0))
                .width(Length::Fixed(560.0))
                .style(theme::input),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center)
        .into(),
        row![
            button(mono("FINISHED", 10.5, INK))
                .on_press(Message::TaskFinish(FinishOutcome::Done))
                .style(theme::primary_button)
                .padding(edge(6.0, 12.0, 6.0, 12.0)),
            button(mono("FAILED", 10.5, INK))
                .on_press(Message::TaskFinish(FinishOutcome::Failed))
                .style(theme::primary_button)
                .padding(edge(6.0, 12.0, 6.0, 12.0)),
            mono("ends the claimed task, with the note above", 10.0, INK_3),
        ]
        .spacing(10.0)
        .align_y(Alignment::Center)
        .into(),
    ])
    .spacing(6.0)
    .into()
}

/// The withdraw row: two presses, armed then confirmed, like a run's kill.
fn task_withdraw_row<'a>(model: &'a Model, task: &'a Task) -> Element<'a, Message> {
    let armed = model.armed_task.as_deref() == Some(task.id.as_str());
    let label = if armed { "CONFIRM WITHDRAW" } else { "WITHDRAW" };
    let mut controls = row![
        button(mono(label, 10.5, if armed { theme::SHEET } else { theme::CRIT }))
            .on_press(Message::TaskWithdrawClicked(task.id.clone()))
            .style(theme::kill_button(armed))
            .padding(edge(6.0, 12.0, 6.0, 12.0)),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);
    if armed {
        controls = controls.push(mono("press again to withdraw", 10.0, theme::CRIT));
    } else {
        controls = controls.push(mono(
            "gives the task up before it ends; the daemon keeps it, withdrawn",
            10.0,
            INK_3,
        ));
    }
    controls.into()
}

/// The ledger's create form: the daemon's four fields and the button that
/// files the task. Nothing is sent until FILE TASK.
fn task_create_form(model: &Model) -> Element<'_, Message> {
    let mut body: Vec<Element<'_, Message>> = vec![
        mono("File a task", 10.5, INK_3).into(),
        rule_heading(),
    ];
    for field in [TaskField::Goal, TaskField::Acceptance, TaskField::Budget, TaskField::Role] {
        body.push(task_form_row(model, field));
    }
    body.push(
        row![
            button(mono("FILE TASK", 11.0, INK))
                .on_press(Message::TaskCreate)
                .style(theme::primary_button)
                .padding(edge(8.0, 14.0, 8.0, 14.0)),
            mono("POST /api/v0/tasks", 10.0, INK_3),
        ]
        .spacing(12.0)
        .align_y(Alignment::Center)
        .into(),
    );
    Column::with_children(body).spacing(6.0).into()
}

/// One create-form row: the column's name and what it holds, the box, and the
/// complaint the last submission made about it.
fn task_form_row<'a>(model: &'a Model, field: TaskField) -> Element<'a, Message> {
    let value = match field {
        TaskField::Goal => model.task_form.goal.as_str(),
        TaskField::Acceptance => model.task_form.acceptance.as_str(),
        TaskField::Budget => model.task_form.budget.as_str(),
        TaskField::Role => model.task_form.role.as_str(),
    };
    let mut right: Vec<Element<'a, Message>> = vec![
        text_input(field.label(), value)
            .on_input(move |value| Message::TaskEdit { field, value })
            .on_submit(Message::TaskCreate)
            .font(MONO)
            .size(12.0)
            .padding(edge(7.0, 9.0, 7.0, 9.0))
            .width(Length::Fixed(420.0))
            .style(theme::input)
            .into(),
    ];
    if let Some(error) = field.error(&model.task_errors) {
        right.push(mono(error, 10.0, theme::CRIT).into());
    }
    row![
        column![
            mono(field.name(), 11.5, INK),
            mono(field.label(), 9.5, INK_3)
        ]
        .width(240),
        Column::with_children(right),
    ]
    .spacing(12.0)
    .align_y(Alignment::Start)
    .into()
}

/// A column the daemon may not have filled: its value, or the register's own
/// dash for nothing.
fn column_or_dash(value: Option<&str>) -> String {
    value.unwrap_or("—").to_string()
}

/// A task's claiming agent's guest CID, as the ledger writes it.
fn agent_cid_label(task: &Task) -> String {
    match task.agent_cid.copied() {
        Some(cid) => format!("cid {cid}"),
        None => "—".to_string(),
    }
}

/// A task's state as a stamp is toned: open waits (neutral ink), claimed is
/// held (the caution), done finished well (green), failed broke (red), and a
/// withdrawal was given up (amber). A state the console does not know is drawn
/// in neutral ink rather than guessed at.
fn tone_for_task(state: TaskState) -> Tone {
    match state {
        TaskState::Open => Tone::Neutral,
        TaskState::Claimed => Tone::Signal,
        TaskState::Done => Tone::Ok,
        TaskState::Failed => Tone::Crit,
        TaskState::Withdrawn => Tone::Warn,
        TaskState::Unknown => Tone::Neutral,
    }
}

/// The session's figures: what the register observed, one observation per
/// poll, as three charts and the tally of the window.
fn telemetry_pane(model: &Model) -> Element<'_, Message> {
    let history = &model.history;
    let gap_ms = model.refresh_ms * history::GAP_FACTOR;
    let pick = model.telemetry_pick();
    let cap = model.machine.as_ref().map(|machine| machine.memory_mib as f32);

    let mut body: Vec<Element<'_, Message>> = vec![
        serif(
            "Every figure on this sheet is the register's own answer to a poll: nothing is \
             estimated, and a poll that fails writes nothing, so a blank stretch in a line is \
             time nobody observed. Throughput counts only the VMs that are still up — a VM that \
             has left the running state keeps a frozen rate in the feed, and counting that \
             would inflate the fleet line forever.",
            12.0,
            INK_2,
        )
        .into(),
        lead(vec![
            ("committed", memory_label(model.committed_mib())),
            ("vms live", model.running_vms().to_string()),
            ("lines/s now", format!("{:.1}", model.fleet_lps())),
            (
                "observations",
                format!("{} kept of {}", history.len(), history.written()),
            ),
        ]),
        telemetry_picker(model),
        // The session's export sits above the charts: a sheet this long is
        // scrolled, and the export is not something to have to scroll to.
        csv_row(model),
    ];

    for kind in [ChartKind::Ram, ChartKind::Vms, ChartKind::Throughput] {
        // The host's own total is a memory figure: it belongs on the RAM
        // chart and nowhere else.
        let cap = (kind == ChartKind::Ram).then_some(cap).flatten();
        body.push(chart_block(model, kind, pick, cap, gap_ms));
    }

    scrollable(Column::with_children(body).spacing(12.0))
        .height(Length::Fill)
        .into()
}

/// One chart: its caption, and the figure itself or the honest note that
/// there is not one yet.
fn chart_block<'a>(
    model: &'a Model,
    kind: ChartKind,
    pick: Option<&'a str>,
    cap: Option<f32>,
    gap_ms: u64,
) -> Element<'a, Message> {
    let samples = model.history.samples();
    let mut rows: Vec<Element<'a, Message>> = vec![mono(chart_caption(kind, model, gap_ms), 10.5, INK_2).into()];
    if samples.len() < history::CHART_MIN_SAMPLES {
        // Fewer than three observations is not a figure: it is a start.
        rows.push(
            serif(
                format!(
                    "only {} observation(s) so far — the figure lands as the register is polled",
                    samples.len()
                ),
                11.5,
                INK_3,
            )
            .into(),
        );
    } else {
        rows.push(
            container(telemetryview::chart(telemetryview::FleetChart {
                samples,
                kind,
                picked: pick,
                cap,
                gap_ms,
            }))
            .style(theme::boxed)
            .width(Length::Fill)
            .height(Length::Fixed(150.0))
            .padding(edge(4.0, 4.0, 4.0, 4.0))
            .into(),
        );
    }
    Column::with_children(rows).spacing(4.0).into()
}

/// What one chart is showing, in prose: the figure, where it stands now, its
/// peak in the window, and how much of the window nobody observed.
fn chart_caption(kind: ChartKind, model: &Model, gap_ms: u64) -> String {
    let samples = model.history.samples();
    let peak = samples
        .iter()
        .map(|sample| kind.value(sample))
        .fold(0.0f32, f32::max);
    let now = model.history.latest().map_or(0.0, |sample| kind.value(sample));
    let mut caption = format!(
        "{} · over {} · now {} · peak {}",
        kind.label(),
        duration_label(model.history.span_ms()),
        kind.tick(now),
        kind.tick(peak)
    );
    match history::gap_count(samples, gap_ms) {
        0 => caption.push_str(" · every poll consecutive"),
        1 => caption.push_str(" · 1 unobserved stretch"),
        gaps => caption.push_str(&format!(" · {gaps} unobserved stretches")),
    }
    caption
}

/// The picks the charts decompose by: the whole fleet, or one run.
fn telemetry_picker(model: &Model) -> Element<'_, Message> {
    let picked = model.telemetry_pick();
    let mut buttons: Vec<Element<'_, Message>> = vec![
        button(mono("the whole fleet", 10.0, INK))
            .on_press(Message::TelemetryPick(None))
            .style(theme::register_row(picked.is_none()))
            .padding(edge(4.0, 9.0, 4.0, 9.0))
            .into(),
    ];
    for run in &model.runs {
        buttons.push(
            button(mono(run.id.as_str(), 10.0, INK))
                .on_press(Message::TelemetryPick(Some(run.id.clone())))
                .style(theme::register_row(picked == Some(run.id.as_str())))
                .padding(edge(4.0, 9.0, 4.0, 9.0))
                .into(),
        );
    }
    let note = match picked {
        Some(run) => picked_line(&model.history, run),
        None => match &model.telemetry_run {
            Some(id) => format!("{id} has left the register: the charts are the whole fleet again"),
            None => "a picked run's band is the one hot stroke; the rest are the register's ink"
                .to_string(),
        },
    };
    Column::with_children(vec![
        Row::with_children(buttons).spacing(4.0).wrap().into(),
        mono(note, 10.0, INK_3).into(),
    ])
    .spacing(4.0)
    .into()
}

/// What a picked run's own slice of the newest observation is.
fn picked_line(history: &history::SessionHistory, run: &str) -> String {
    let Some(sample) = history.latest() else {
        return format!("picked {run} · nothing observed yet");
    };
    let Some(slice) = sample.runs.iter().find(|slice| slice.run == run) else {
        return format!("picked {run} · not in the newest observation");
    };
    format!(
        "picked {run} · {} live · {} committed · {:.1} lines/s now",
        slice.vms,
        memory_label(slice.ram_mib),
        slice.lps
    )
}

/// The session export: the CSV's path, and the button that writes it, through
/// the same one write the transcript uses.
fn csv_row(model: &Model) -> Element<'_, Message> {
    Column::with_children(vec![
        row![
            mono("Export the session (CSV)", 10.5, INK_3),
            text_input("a path to write the session's CSV to", &model.csv.path)
                .on_input(Message::CsvEdited)
                .font(MONO)
                .size(11.5)
                .padding(edge(5.0, 8.0, 5.0, 8.0))
                .width(Length::Fill)
                .style(theme::input),
            button(mono("EXPORT CSV", 10.5, INK))
                .on_press(Message::CsvExport)
                .style(theme::primary_button)
                .padding(edge(6.0, 12.0, 6.0, 12.0)),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center)
        .into(),
        mono(csv_line(model), 10.0, INK_3).into(),
    ])
    .spacing(4.0)
    .into()
}

/// What the CSV button will write, and where: one row per observation for the
/// fleet and one for each of its runs, and never anything the guest wrote.
fn csv_line(model: &Model) -> String {
    let path = match model.csv.path.trim() {
        "" => "— (no path)".to_string(),
        path => path.to_string(),
    };
    let samples = model.history.len() as u64;
    let runs: u64 = model
        .history
        .samples()
        .iter()
        .map(|sample| sample.runs.len() as u64)
        .sum();
    format!(
        "{} fleet row(s) + {runs} run row(s) · observed_at, elapsed_s, scope, vms_live, \
         ram_mib, lines_per_s → {path}",
        samples
    )
}

/// One run: its facts, its VMs, its controls, its console and its serial.
fn run_pane<'a>(model: &'a Model, id: &str) -> Element<'a, Message> {
    let Some(run) = model.detail.as_ref().filter(|run| run.id == id) else {
        return serif("fetching the run…", 12.5, INK_2).into();
    };

    let mut vm_rows: Vec<Element<'a, Message>> = vec![
        row![
            mono("vm", 10.0, INK_3).width(Length::Fixed(46.0)),
            mono("role", 10.0, INK_3).width(Length::Fixed(150.0)),
            mono("state", 10.0, INK_3).width(Length::Fixed(90.0)),
            mono("guest cid", 10.0, INK_3).width(Length::Fixed(96.0)),
            mono("exit", 10.0, INK_3).width(Length::Fixed(70.0)),
            mono("rate", 10.0, INK_3).width(Length::Fixed(84.0)),
            mono("serial", 10.0, INK_3),
        ]
        .spacing(8.0)
        .into(),
        rule::horizontal(1.0).style(theme::rule_ink).into(),
    ];
    for vm in &run.vms {
        vm_rows.push(vm_row(model, run, vm));
    }

    Column::with_children(vec![
        lead(vec![
            ("state", run.state.label().to_string()),
            ("vms", format!("{} of {} up", run.live_vms(), run.vms.len())),
            ("vcpus", run.vcpus.to_string()),
            ("memory", format!("{} MiB", run.memory_mib)),
            (
                "beats",
                match run.beats {
                    Some(beats) => beats.to_string(),
                    None => "long-lived".to_string(),
                },
            ),
            (
                "vsock",
                match run.base_cid() {
                    Some(cid) => format!("{cid}+"),
                    None => "none".to_string(),
                },
            ),
            ("created", instant_label(run.created_at, model.run_reference_ms(run))),
            ("elapsed", duration_label(model.run_elapsed_ms(run))),
            (
                "ended",
                match run.ended_at {
                    Some(ended) => instant_label(ended, model.now_ms),
                    None => "—".to_string(),
                },
            ),
        ]),
        controls(model, run),
        Column::with_children(vm_rows).spacing(4.0).into(),
        rate_pane(model, run),
        console(model, run),
        export_row(model, run),
        serial_pane(model, run),
    ])
    .spacing(12.0)
    .height(Length::Fill)
    .into()
}

/// The selected VM's own serial rate over the session, as a sparkline.
///
/// One chart for the VM the console targets rather than one in every row: the
/// run pane can hold fifty rows (the scale preset), and an iced canvas per row
/// is a geometry, a program and a redraw each, all of them rebuilt every poll
/// and none of them reachable by the register's own hit-testing. The numeric
/// rate stays in every row, so nothing is lost by charting the one VM the
/// operator is actually reading.
fn rate_pane<'a>(model: &'a Model, run: &'a Run) -> Element<'a, Message> {
    let target = model.target_vm(&run.id);
    let series = model.history.vm_series(&run.id, target);
    let live = run
        .vms
        .iter()
        .find(|vm| vm.id == target)
        .is_some_and(|vm| vm.state == VmState::Running);

    let caption = if series.is_empty() {
        format!("vm {target} · no observation yet")
    } else {
        let peak = series.iter().copied().fold(0.0f32, f32::max);
        format!(
            "vm {target} · now {:.1} · peak {peak:.1} · {} point(s) of {}",
            series.last().copied().unwrap_or(0.0),
            series.len(),
            history::HISTORY_CAP
        )
    };

    let mut rows: Vec<Element<'a, Message>> = vec![
        row![
            mono(format!("Serial rate · vm {target} (lines/s)"), 10.5, INK_3),
            space::horizontal(),
            mono(caption, 10.0, INK_3),
        ]
        .align_y(Alignment::Center)
        .into(),
    ];
    if series.len() < history::CHART_MIN_SAMPLES {
        rows.push(serif("too few observations to draw a shape yet", 11.5, INK_3).into());
    } else {
        rows.push(
            container(telemetryview::sparkline(telemetryview::Sparkline {
                points: series,
                live,
            }))
            .style(theme::boxed)
            .width(Length::Fill)
            .height(Length::Fixed(28.0))
            .padding(edge(2.0, 4.0, 2.0, 4.0))
            .into(),
        );
    }
    Column::with_children(rows).spacing(4.0).into()
}

/// The follow, refresh and kill controls of a run.
fn controls<'a>(model: &'a Model, run: &'a Run) -> Element<'a, Message> {
    let armed = model.armed.as_deref() == Some(run.id.as_str());
    let follow = if model.follow { "FOLLOW ON" } else { "FOLLOW OFF" };
    let mut controls = row![
        button(mono(follow, 10.5, INK))
            .on_press(Message::Follow(!model.follow))
            .style(theme::primary_button)
            .padding(edge(6.0, 10.0, 6.0, 10.0)),
        button(mono("REFRESH", 10.5, INK))
            .on_press(Message::Show(Pane::Run(run.id.clone())))
            .style(theme::primary_button)
            .padding(edge(6.0, 10.0, 6.0, 10.0)),
    ]
    .spacing(8.0)
    .align_y(Alignment::Center);

    if run.state.is_live() || armed {
        let label = if armed { "CONFIRM KILL" } else { "KILL" };
        controls = controls.push(
            button(mono(label, 10.5, if armed { theme::SHEET } else { theme::CRIT }))
                .on_press(Message::KillClicked(run.id.clone()))
                .style(theme::kill_button(armed))
                .padding(edge(6.0, 10.0, 6.0, 10.0)),
        );
    }
    if armed {
        controls = controls.push(mono("press again to kill", 10.0, theme::CRIT));
    }
    controls.into()
}

/// One VM's line: its state, its guest CID, how it left, its serial rate and
/// the last line it wrote. Pressing it aims the console at that VM.
fn vm_row<'a>(model: &'a Model, run: &'a Run, vm: &'a Vm) -> Element<'a, Message> {
    let cid = match vm.guest_cid {
        Some(cid) => format!("cid {cid}"),
        None => "no vsock".to_string(),
    };
    let serial = match vm.serial.as_deref() {
        Some(serial) => format!("{} bytes", serial.len()),
        None => "in the run pane".to_string(),
    };
    let rate = model.activity.lps(&run.id, vm.id);
    let rate = if rate > 0.02 {
        format!("{rate:.1} ln/s")
    } else {
        "—".to_string()
    };
    // A VM that is still up has no exit to read; the tail belongs to its
    // session in progress.
    let exit = if vm.state == VmState::Running {
        None
    } else {
        vm.serial.as_deref().map(exit_kind)
    };
    let target = model.target_vm(&run.id) == vm.id;

    let mut cells: Vec<Element<'a, Message>> = vec![
        mono(format!("vm {:02}", vm.id), 11.0, INK).width(Length::Fixed(46.0)).into(),
    ];
    // The role a VM's guest runs as, stamped where a VM is listed: an agent
    // is who it is, so a role agent's row says so, and a VM that is not one
    // gets the register's dash and no stamp at all.
    cells.push(match vm.role.as_deref() {
        Some(role) => stamp(role.to_uppercase(), Tone::Neutral)
            .width(Length::Fixed(150.0))
            .into(),
        None => mono("—", 10.5, INK_3)
            .width(Length::Fixed(150.0))
            .into(),
    });
    cells.push(
        stamp(vm.state.stamp_label(), tone_for_vm(vm.state))
            .width(Length::Fixed(90.0))
            .into(),
    );
    cells.push(mono(cid, 10.5, INK_2).width(Length::Fixed(96.0)).into());
    cells.push(match exit {
        Some(kind) => stamp(kind.stamp_label(), tone_for_exit(kind))
            .width(Length::Fixed(70.0))
            .into(),
        None => mono("—", 10.5, INK_3).width(Length::Fixed(70.0)).into(),
    });
    cells.push(mono(rate, 10.5, INK_2).width(Length::Fixed(84.0)).into());
    let latest = model
        .latest_line(vm)
        .map(|line| format!("{serial} · {line}"))
        .unwrap_or(serial);
    cells.push(mono(latest, 10.5, INK_3).wrapping(Wrapping::None).into());

    button(Row::with_children(cells).spacing(8.0).align_y(Alignment::Center))
        .on_press(Message::SelectVm(vm.id))
        .style(theme::register_row(target))
        .padding(edge(4.0, 6.0, 4.0, 4.0))
        .width(Length::Fill)
        .into()
}

/// The console: a line for the guest, the guest's own commands as buttons, and
/// the history the arrows walk.
fn console<'a>(model: &'a Model, run: &'a Run) -> Element<'a, Message> {
    let target = model.target_vm(&run.id);
    let mut rows: Vec<Element<'a, Message>> = vec![
        row![
            mono("Write to the serial console", 10.5, INK_3),
            space::horizontal(),
            mono(
                match run.vms.iter().find(|vm| vm.id == target).and_then(|vm| vm.guest_cid) {
                    Some(cid) => format!("vm {target} · cid {cid}"),
                    None => format!("vm {target} · no vsock"),
                },
                10.0,
                INK_2,
            ),
        ]
        .align_y(Alignment::Center)
        .into(),
    ];

    let mut quick: Vec<Element<'a, Message>> = Vec::with_capacity(QUICK_COMMANDS.len());
    for command in QUICK_COMMANDS {
        quick.push(
            button(mono(command, 10.0, INK))
                .on_press(Message::ConsoleQuick(command))
                .style(theme::primary_button)
                .padding(edge(4.0, 9.0, 4.0, 9.0))
                .into(),
        );
    }
    rows.push(Row::with_children(quick).spacing(6.0).into());

    let input = text_input("a line for the guest, newline added", model.console.line.as_str())
        .on_input(Message::ConsoleEdit)
        .on_submit(Message::ConsoleSend)
        .font(MONO)
        .size(12.0)
        .padding(edge(7.0, 9.0, 7.0, 9.0))
        .width(Length::Fill)
        .style(theme::input);
    rows.push(
        row![
            input,
            button(mono("SEND", 10.5, INK))
                .on_press(Message::ConsoleSend)
                .style(theme::primary_button)
                .padding(edge(6.0, 12.0, 6.0, 12.0)),
            button(mono("↑", 11.5, INK_2))
                .on_press(Message::ConsoleHistory(HistoryStep::Prev))
                .style(theme::primary_button)
                .padding(edge(6.0, 10.0, 6.0, 10.0)),
            button(mono("↓", 11.5, INK_2))
                .on_press(Message::ConsoleHistory(HistoryStep::Next))
                .style(theme::primary_button)
                .padding(edge(6.0, 10.0, 6.0, 10.0)),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center)
        .into(),
    );
    rows.push(serif(
        if run.state.is_live() {
            "The guest takes one line at a time and answers between heartbeats; the reply \
             lands in the transcript below. It understands: help, uname, cpu, mem, uptime, \
             echo <text>, poweroff."
        } else {
            "The run is not running: the daemon refuses console input, and says so in the \
             notice line."
        },
        11.5,
        INK_3,
    )
    .into());

    Column::with_children(rows).spacing(6.0).into()
}

/// The transcript export: what it covers, where it goes, and the button that
/// writes it. The path is a box (iced offers no file dialog and the crate
/// takes no dependency for one), pre-filled with a default in the temp
/// directory.
fn export_row<'a>(model: &'a Model, run: &'a Run) -> Element<'a, Message> {
    let mut toggles: Vec<Element<'a, Message>> = Vec::new();
    for target in ExportTarget::ALL {
        toggles.push(
            button(mono(target.label(), 10.0, INK))
                .on_press(Message::ExportTarget(target))
                .style(theme::register_row(target == model.export.target))
                .padding(edge(4.0, 9.0, 4.0, 9.0))
                .into(),
        );
    }

    Column::with_children(vec![
        row![
            mono("Export the transcript", 10.5, INK_3),
            Row::with_children(toggles).spacing(4.0),
            text_input("a path to write the transcript to", &model.export.path)
                .on_input(Message::ExportEdited)
                .font(MONO)
                .size(11.5)
                .padding(edge(5.0, 8.0, 5.0, 8.0))
                .width(Length::Fill)
                .style(theme::input),
            button(mono("EXPORT", 10.5, INK))
                .on_press(Message::Export)
                .style(theme::primary_button)
                .padding(edge(6.0, 12.0, 6.0, 12.0)),
        ]
        .spacing(8.0)
        .align_y(Alignment::Center)
        .into(),
        mono(export_line(model, run), 10.0, INK_3).into(),
    ])
    .spacing(4.0)
    .into()
}

/// What the export button will write, and where: the box names one path, so
/// the line under it says exactly what lands there.
fn export_line(model: &Model, run: &Run) -> String {
    let path = match model.export.path.trim() {
        "" => "— (no path)".to_string(),
        path => path.to_string(),
    };
    // The captures the write will carry: one VM's, or every VM's.
    let capture_bytes = |only: Option<u32>| -> usize {
        run.vms
            .iter()
            .filter(|vm| match only {
                Some(id) => vm.id == id,
                None => true,
            })
            .filter_map(|vm| vm.serial.as_deref())
            .map(str::len)
            .sum()
    };
    match model.export.target {
        ExportTarget::Vm => {
            let vm = model.target_vm(&run.id);
            format!("vm {vm} · {} byte(s) of capture → {path}", capture_bytes(Some(vm)))
        }
        ExportTarget::Run => format!(
            "every vm of {} · {} section(s) · {} byte(s) of capture → {path}",
            run.id,
            run.vms.len(),
            capture_bytes(None)
        ),
        ExportTarget::Report => format!(
            "the report of {} · {} section(s) · {} byte(s) of capture → {path}",
            run.id,
            run.vms.len(),
            capture_bytes(None)
        ),
    }
}

/// The serial transcript: the last lines of the VM the console is aimed at,
/// every one of them numbered with the byte of the capture it starts at.
///
/// The margin's numbers are the register's own ledger habit — the draft's
/// transcript puts each row's sequence number in the margin and names the
/// span above it — over the figure this console has: the byte offsets the
/// stream's `serial` events carry, counted over the capture the poll read
/// whole. A transcript that only ever grows reads as offsets that only ever
/// go up.
fn serial_pane<'a>(model: &'a Model, run: &'a Run) -> Element<'a, Message> {
    let target = model.target_vm(&run.id);
    let capture = model.target_capture(run);
    let (skipped, rows) = serial_tail(capture, SERIAL_TAIL_LINES);

    let heading = match transcript_span(capture, &rows) {
        Some((from, to)) => format!(
            "Serial · vm {target} · {} line(s) · byte {from}–{to}",
            rows.len()
        ),
        None => format!("Serial · vm {target} · no bytes yet"),
    };

    let mut body: Vec<Element<'a, Message>> = Vec::with_capacity(rows.len() + 2);
    if capture.is_empty() {
        body.push(serif("the guest has not written to its console yet", 12.0, INK_3).into());
    }
    if skipped > 0 {
        body.push(mono(format!("… {skipped} earlier line(s) not shown"), 10.0, INK_3).into());
    }
    for row in &rows {
        body.push(
            row![
                mono(row.offset.to_string(), 9.5, INK_3)
                    .width(Length::Fixed(58.0))
                    .align_x(iced::alignment::Horizontal::Right),
                mono(row.text, 11.0, INK),
            ]
            .spacing(8.0)
            .into(),
        );
    }

    let transcript = scrollable(Column::with_children(body).spacing(1.0).width(Length::Fill))
        .height(Length::Fill)
        .auto_scroll(model.follow);

    Column::with_children(vec![
        mono(heading, 10.5, INK_3).into(),
        container(transcript)
            .style(theme::boxed)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(edge(10.0, 10.0, 10.0, 10.0))
            .into(),
    ])
    .spacing(4.0)
    .height(Length::Fill)
    .into()
}

/// The quick-boot presets: three buttons and the footprint each one asks the
/// host for. A press only fills the form — the numbers stay visible and
/// adjustable before anything is sent.
fn presets() -> Element<'static, Message> {
    let mut columns: Vec<Element<'static, Message>> = Vec::with_capacity(Preset::ALL.len());
    for preset in Preset::ALL {
        columns.push(
            column![
                button(mono(preset.label(), 11.0, INK))
                    .on_press(Message::Preset(preset))
                    .style(theme::primary_button)
                    .padding(edge(7.0, 12.0, 7.0, 12.0))
                    .width(Length::Fill),
                mono(preset.what(), 9.5, INK_3),
                mono(preset_footprint(preset), 9.5, INK_2),
            ]
            .spacing(4.0)
            .width(Length::FillPortion(1))
            .into(),
        );
    }
    Row::with_children(columns).spacing(14.0).into()
}

/// The spawn form.
fn spawn_pane(model: &Model) -> Element<'_, Message> {
    let mut body: Vec<Element<'_, Message>> = vec![
        serif(
            "The daemon's own ranges: n and vcpus are 1–64, memory_mib is at \
             least 128, vsock_cid starts at 3 (1 and 2 are reserved).",
            12.5,
            INK_2,
        )
        .into(),
        rule::horizontal(1.0).style(theme::rule_ink).into(),
        mono("Quick boots — each one fills the form below; nothing is sent until BOOT RUN", 10.5, INK_3).into(),
        presets(),
        vertical(6.0),
        rule::horizontal(1.0).style(theme::rule).into(),
    ];
    for field in Field::ALL {
        body.push(form_row(model, field));
    }
    body.push(vertical(4.0));
    body.push(
        row![
            button(mono("BOOT RUN", 11.0, INK))
                .on_press(Message::SpawnClicked)
                .style(theme::primary_button)
                .padding(edge(8.0, 14.0, 8.0, 14.0)),
            mono("POST /api/v0/runs", 10.0, INK_3),
        ]
        .spacing(12.0)
        .align_y(Alignment::Center)
        .into(),
    );
    Column::with_children(body).spacing(12.0).into()
}

/// One form row: the field's name and range on the left, the box and any
/// error on the right.
fn form_row<'a>(model: &'a Model, field: Field) -> Element<'a, Message> {
    let input = text_input(field.hint(), model.form.field(field))
        .on_input(move |value| Message::Edit { field, value })
        .on_submit(Message::SpawnClicked)
        .font(MONO)
        .size(12.0)
        .padding(edge(7.0, 9.0, 7.0, 9.0))
        .width(Length::Fixed(320.0))
        .style(theme::input);

    let mut right: Vec<Element<'a, Message>> = vec![input.into()];
    if let Some(error) = model.error_for(field) {
        right.push(mono(error, 10.0, theme::CRIT).into());
    }

    row![
        column![
            mono(field.name(), 11.5, INK),
            mono(field.hint(), 9.5, INK_3)
        ]
        .width(220),
        Column::with_children(right),
    ]
    .spacing(12.0)
    .align_y(Alignment::Start)
    .into()
}

/// A key/value block: the run's facts, in the register's lead style.
fn lead(facts: Vec<(&'static str, String)>) -> Element<'static, Message> {
    let mut cells: Vec<Element<'static, Message>> = Vec::with_capacity(facts.len());
    for (key, value) in facts {
        cells.push(
            column![mono(key, 9.5, INK_3), mono(value, 11.5, INK)]
                .width(Length::FillPortion(1))
                .into(),
        );
    }
    Row::with_children(cells).spacing(10.0).into()
}

/// A stamped word, boxed in its tone. The word may be static (a state's own
/// label) or borrowed (a role, which is the daemon's word and not a constant).
fn stamp<'a>(word: impl IntoFragment<'a>, tone: Tone) -> Container<'a, Message> {
    container(mono(word, 9.5, tone.color()))
        .style(theme::stamp(tone))
        .padding(edge(3.0, 7.0, 3.0, 7.0))
}

/// The monospace voice.
fn mono<'a>(value: impl IntoFragment<'a>, size: f32, color: iced::Color) -> Text<'a> {
    text(value).font(MONO).size(size).color(color)
}

/// The serif voice: prose, set against the mono data.
fn serif<'a>(value: impl IntoFragment<'a>, size: f32, color: iced::Color) -> Text<'a> {
    text(value).font(SERIF).size(size).color(color)
}

/// A blank line of `height`.
fn vertical(height: f32) -> Element<'static, Message> {
    space::vertical().height(Length::Fixed(height)).into()
}

/// A ruled line under a heading.
fn rule_heading<'a>() -> Element<'a, Message> {
    rule::horizontal(1.0).style(theme::rule).into()
}

/// Padding around a widget.
fn edge(top: f32, right: f32, bottom: f32, left: f32) -> Padding {
    Padding {
        top,
        right,
        bottom,
        left,
    }
}

/// The tone a run's state is stamped in.
fn tone_for_run(state: RunState) -> Tone {
    match state {
        RunState::Starting => Tone::Signal,
        RunState::Running => Tone::Ok,
        RunState::Done => Tone::Neutral,
        RunState::Failed => Tone::Crit,
        RunState::Killed => Tone::Warn,
        RunState::Unknown => Tone::Neutral,
    }
}

/// The tone a VM's state is stamped in.
fn tone_for_vm(state: VmState) -> Tone {
    match state {
        VmState::Running => Tone::Ok,
        VmState::Exited => Tone::Neutral,
        VmState::Killed => Tone::Warn,
        VmState::Unknown => Tone::Neutral,
    }
}

/// The tone a VM's exit kind is stamped in: a clean sleep finished well, a
/// reset is worth watching, and an unread tail says nothing.
fn tone_for_exit(kind: ExitKind) -> Tone {
    match kind {
        ExitKind::Slept => Tone::Ok,
        ExitKind::Reset => Tone::Warn,
        ExitKind::Unknown => Tone::Neutral,
    }
}

/// The tone the connection is stamped in.
fn tone_for_health(health: Health) -> Tone {
    match health {
        Health::Unknown => Tone::Neutral,
        Health::Live => Tone::Ok,
        Health::Stale => Tone::Warn,
        Health::Down => Tone::Crit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Vm;
    use crate::form::SpawnForm;

    fn machine() -> crate::api::Machine {
        crate::api::Machine {
            name: "zarathos-pc".to_string(),
            arch: "x86_64".to_string(),
            backend: "kvm".to_string(),
            irqchip: "ioapic".to_string(),
            class: "server".to_string(),
            cpus: 24,
            memory_mib: 128_320,
            vsock_mode: "uds-muxer".to_string(),
            started_at_ms: 1_760_000_000_000,
        }
    }

    fn run() -> Run {
        Run {
            id: "r1".to_string(),
            state: RunState::Running,
            vcpus: 2,
            memory_mib: 512,
            beats: None,
            created_at: 1_760_000_000_000,
            ended_at: None,
            vms: vec![Vm {
                id: 0,
                guest_cid: Some(50),
                role: None,
                state: VmState::Running,
                serial: Some("$ uname\nLinux\n".to_string()),
            }],
        }
    }

    #[test]
    fn every_state_has_a_stamp_and_a_tone() {
        for state in [
            RunState::Starting,
            RunState::Running,
            RunState::Done,
            RunState::Failed,
            RunState::Killed,
            RunState::Unknown,
        ] {
            assert!(!state.stamp_label().is_empty());
            let _ = tone_for_run(state);
        }
        for state in [
            VmState::Running,
            VmState::Exited,
            VmState::Killed,
            VmState::Unknown,
        ] {
            assert!(!state.stamp_label().is_empty());
            let _ = tone_for_vm(state);
        }
        for health in [Health::Unknown, Health::Live, Health::Stale, Health::Down] {
            assert!(!health.stamp_label().is_empty());
            let _ = tone_for_health(health);
        }
        for kind in [ExitKind::Slept, ExitKind::Reset, ExitKind::Unknown] {
            assert!(!kind.stamp_label().is_empty());
            let _ = tone_for_exit(kind);
        }
        // A task's five states, plus the unknown one a newer daemon may send:
        // every one has a stamp, and every one a tone.
        for (state, tone) in [
            (TaskState::Open, Tone::Neutral),
            (TaskState::Claimed, Tone::Signal),
            (TaskState::Done, Tone::Ok),
            (TaskState::Failed, Tone::Crit),
            (TaskState::Withdrawn, Tone::Warn),
            (TaskState::Unknown, Tone::Neutral),
        ] {
            assert!(!state.stamp_label().is_empty());
            assert_eq!(tone_for_task(state), tone);
        }
    }

    /// The ledger's columns, summed: what its table occupies before spacing.
    /// It lives here because the check that reads it is what it is for.
    const TASK_TABLE: f32 = TASK_COL_ID
        + TASK_COL_GOAL
        + TASK_COL_ROLE
        + TASK_COL_BUDGET
        + TASK_COL_STATE
        + TASK_COL_ASSIGNEE
        + TASK_COL_CID
        + TASK_COL_STAMP
        + TASK_COL_STAMP;

    #[test]
    fn the_ledgers_columns_fit_the_sheet_the_window_has() {
        // The ledger's table is fixed-width columns, and each is wide enough
        // for the widest value it prints. What is left of the window for it is
        // the width less the index's own fixed width and both paddings — and a
        // column widened past that is a row that wraps, which stops the row
        // being pressable at all (a wrapped row has no whole-row hit area, and
        // iced draws a zero-width one).
        const WINDOW: f32 = 1280.0;
        const INDEX: f32 = 280.0;
        const INDEX_PADDING: f32 = 36.0;
        const SHEET_PADDING: f32 = 44.0;
        const SPACING: f32 = COL_SPACING * 9.0;
        let sheet = WINDOW - INDEX - INDEX_PADDING - SHEET_PADDING;
        assert!(
            TASK_TABLE + SPACING <= sheet,
            "the ledger's table is {TASK_TABLE} wide plus {SPACING} of spacing, and the sheet has {sheet}"
        );
    }

    #[test]
    fn every_pane_builds_its_tree() {
        let mut model = Model::new();
        model.runs = vec![run()];
        model.detail = Some(run());
        model.machine = Some(machine());
        model.selection = Some(crate::model::Selection {
            run: "r1".to_string(),
            vm: 0,
        });
        model.polls = 3;

        for pane in [
            Pane::Fleet,
            Pane::Run("r1".to_string()),
            Pane::Spawn,
            Pane::Machine,
            Pane::Map,
            Pane::Roster,
            Pane::Ledger,
            Pane::Telemetry,
        ] {
            model.pane = pane;
            let _: Element<'_, Message> = view(&model);
        }
    }

    #[test]
    fn the_telemetry_pane_names_its_figures_and_its_quiet_note() {
        let mut model = Model::new();
        model.pane = Pane::Telemetry;

        // Nothing observed yet: every chart says so instead of drawing junk.
        assert_eq!(chart_caption(ChartKind::Ram, &model, 4_000), "committed guest RAM · over 0s · now 0 MiB · peak 0 MiB · every poll consecutive");
        assert!(csv_line(&model).contains("0 fleet row(s)"));
        let _ = view(&model);

        // Two observations: still a start, not a figure.
        for (at_ms, live) in [(1_000u64, 1u32), (3_000, 2)] {
            let runs = vec![Run {
                vms: (0..live)
                    .map(|id| Vm {
                        id,
                        guest_cid: Some(50 + u64::from(id)),
                        role: None,
                        state: VmState::Running,
                        serial: None,
                    })
                    .collect(),
                ..run()
            }];
            model.apply(Message::Runs {
                at_ms,
                result: Ok(crate::api::RunsPage { runs }),
            });
        }
        assert_eq!(model.history.len(), 2);
        assert!(model.history.len() < history::CHART_MIN_SAMPLES);
        assert_eq!(
            chart_caption(ChartKind::Ram, &model, 4_000),
            "committed guest RAM · over 2s · now 1.0 GiB · peak 1.0 GiB · every poll consecutive"
        );
        let _ = view(&model);

        // Three: the figure lands, and the caption says what it covers.
        model.apply(Message::Runs {
            at_ms: 5_000,
            result: Ok(crate::api::RunsPage {
                runs: vec![Run {
                    vms: vec![Vm {
                        id: 0,
                        guest_cid: Some(50),
                        role: None,
                        state: VmState::Running,
                        serial: None,
                    }],
                    ..run()
                }],
            }),
        });
        assert!(model.history.len() >= history::CHART_MIN_SAMPLES);
        assert_eq!(
            chart_caption(ChartKind::Vms, &model, 4_000),
            "live vms · over 4s · now 1 · peak 2 · every poll consecutive"
        );
        // A picked run's own slice, and the figure the charts decompose by.
        model.telemetry_run = Some("r1".to_string());
        assert_eq!(model.telemetry_pick(), Some("r1"));
        assert!(
            picked_line(&model.history, "r1").starts_with("picked r1 · 1 live"),
            "{:?}",
            picked_line(&model.history, "r1")
        );
        let _ = view(&model);

        // A poll four cadences later is unobserved time, and the caption says
        // how many stretches there were.
        model.apply(Message::Runs {
            at_ms: 30_000,
            result: Ok(crate::api::RunsPage { runs: vec![] }),
        });
        assert!(chart_caption(ChartKind::Vms, &model, 4_000).ends_with("1 unobserved stretch"));
        assert_eq!(history::gap_count(model.history.samples(), 4_000), 1);
        assert_eq!(csv_line(&model), "4 fleet row(s) + 3 run row(s) · observed_at, elapsed_s, scope, vms_live, ram_mib, lines_per_s → /tmp/plaza-telemetry-1000.csv");
        let _ = view(&model);

        // A pick that is in the register is decomposed; one that has gone says
        // so and the charts fall back to the whole fleet.
        model.telemetry_run = Some("r9".to_string());
        assert_eq!(model.telemetry_pick(), None, "r9 is not in the register");
        assert_eq!(
            picked_line(&model.history, "r9"),
            "picked r9 · not in the newest observation"
        );
        let _ = view(&model);
    }

    #[test]
    fn the_machine_and_map_panes_build_with_and_without_a_machine() {
        let mut model = Model::new();
        for pane in [Pane::Machine, Pane::Map, Pane::Telemetry] {
            model.pane = pane;
            let _ = view(&model);
            model.machine = Some(machine());
            let _ = view(&model);
        }
        // A refused console line and a run with a tail both render.
        model.pane = Pane::Run("r1".to_string());
        let mut detail = run();
        detail.vms[0].state = VmState::Exited;
        detail.vms[0].serial = Some("$ poweroff\nreboot: Power down\n".to_string());
        model.detail = Some(detail);
        model.notice = Some(Notice {
            kind: NoticeKind::Failed,
            text: "console r1 vm 0: 409: the run is killed".to_string(),
        });
        let _ = view(&model);
    }

    #[test]
    fn a_run_that_has_not_arrived_renders_a_placeholder() {
        let mut model = Model::new();
        model.pane = Pane::Run("r7".to_string());
        let _ = view(&model);
    }

    #[test]
    fn an_empty_fleet_and_a_notice_render() {
        let mut model = Model::new();
        assert_eq!(model.health(), Health::Unknown);
        model.notice = Some(Notice {
            kind: NoticeKind::Failed,
            text: "spawn refused: 400: n must be between 1 and 64".to_string(),
        });
        let _ = view(&model);
    }

    #[test]
    fn the_form_renders_its_errors_and_its_defaults() {
        let mut model = Model::new();
        model.pane = Pane::Spawn;
        model.errors = vec![(
            Field::MemoryMib,
            "memory_mib must be at least 128".to_string(),
        )];
        let _ = view(&model);

        let form = SpawnForm::default();
        assert_eq!(form.field(Field::N), "1");
        assert_eq!(form.field(Field::VsockCid), "");
    }

    #[test]
    fn the_key_table_is_one_source_for_the_index_and_the_help_sheet() {
        let keys: Vec<&str> = KEYS.iter().map(|(key, _)| *key).collect();
        for bound in ["r", "w", "l", "m", "f", "t", "?", "esc", "↑ ↓"] {
            assert!(keys.contains(&bound), "the register binds {bound:?}");
        }
        for (_, what) in KEYS {
            assert!(!what.is_empty(), "every key says what it does");
        }
    }

    #[test]
    fn the_help_sheet_renders_and_says_what_the_quick_commands_are_not() {
        let mut model = Model::new();
        let sheet = help_sheet();
        let _: Element<'_, Message> = sheet;

        model.help = true;
        let _ = view(&model);
        assert!(model.help, "building the tree changes nothing");
    }

    #[test]
    fn the_export_line_names_the_target_and_where_it_lands() {
        let mut model = Model::new();
        model.pane = Pane::Run("r1".to_string());
        let mut detail = run();
        detail.vms[0].serial = Some("login: plaza\n".to_string());
        detail.vms.push(Vm {
            id: 1,
            guest_cid: Some(51),
            role: None,
            state: VmState::Running,
            serial: Some("up\n".to_string()),
        });
        model.detail = Some(detail.clone());
        model.export.path = "/tmp/plaza-r1-0.txt".to_string();

        assert_eq!(
            export_line(&model, &detail),
            "vm 0 · 13 byte(s) of capture → /tmp/plaza-r1-0.txt"
        );

        model.export.target = ExportTarget::Run;
        assert_eq!(
            export_line(&model, &detail),
            "every vm of r1 · 2 section(s) · 16 byte(s) of capture → /tmp/plaza-r1-0.txt"
        );

        model.export.target = ExportTarget::Report;
        assert_eq!(
            export_line(&model, &detail),
            "the report of r1 · 2 section(s) · 16 byte(s) of capture → /tmp/plaza-r1-0.txt"
        );

        // A VM whose serial the pane has not seen counts nothing.
        model.export.target = ExportTarget::Vm;
        model.selection = Some(crate::model::Selection {
            run: "r1".to_string(),
            vm: 7,
        });
        assert_eq!(
            export_line(&model, &detail),
            "vm 7 · 0 byte(s) of capture → /tmp/plaza-r1-0.txt"
        );
    }

    #[test]
    fn the_preset_names_and_footprints_are_the_static_labels_the_form_carries() {
        let labels: Vec<&str> = Preset::ALL.iter().map(|preset| preset.label()).collect();
        assert_eq!(labels, vec!["2 VMs + vsock", "8 VMs", "50-VM scale"]);
        assert_eq!(
            preset_footprint(Preset::Scale),
            "50 × 128 MiB ≈ 6.3 GiB guest RAM"
        );
        for preset in Preset::ALL {
            assert!(!preset.what().is_empty());
        }
    }
}
