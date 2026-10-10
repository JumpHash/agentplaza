//! The register as iced draws it: an index on the left, a ruled sheet on the
//! right, one hot signal, and stamps for every state the daemon reports.
//!
//! Every pane is a pure function of the [`Model`], so what the window shows is
//! what the headless reducer decided.

use iced::widget::text::{IntoFragment, Wrapping};
use iced::widget::{
    Column, Container, Row, Text, button, column, container, row, rule, scrollable, space, text,
    text_input,
};
use iced::{Alignment, Element, Length, Padding};

use crate::api::{Run, RunState, Vm, VmState};
use crate::form::Field;
use crate::model::{
    Health, Message, Model, Notice, NoticeKind, Pane, SERIAL_TAIL_LINES, clock_label,
    duration_label, serial_tail,
};
use crate::theme::{self, INK, INK_2, INK_3, MONO, SERIF, Tone};

/// The whole register.
pub fn view(model: &Model) -> Element<'_, Message> {
    container(row![index(model), sheet(model)])
        .style(theme::paper)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The index column: the brand, the fleet, the runs, and where the register
/// stands with the daemon.
fn index(model: &Model) -> Container<'_, Message> {
    let (runs, vms) = model.fleet_tally();
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
            format!("{runs} run(s) · {vms} vm(s) up"),
            Pane::Fleet,
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
    entries.push(vertical(16.0));
    entries.push(rule_heading());
    entries.push(
        mono(model.endpoint.url(), 10.0, INK_2)
            .wrapping(Wrapping::None)
            .into(),
    );
    entries.push(
        mono(
            format!("polls {} · {}", model.polls, model.health().label()),
            10.0,
            INK_3,
        )
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
            "run", "state", "vms", "vcpus", "memory", "vsock", "created",
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

/// One run's row, which opens the run when pressed.
fn fleet_row<'a>(model: &'a Model, run: &'a Run) -> Element<'a, Message> {
    let selected = model.pane == Pane::Run(run.id.clone());
    let vms = format!("{}/{} up", run.live_vms(), run.vms.len());
    let vsock = match run.base_cid() {
        Some(cid) => format!("{cid}+"),
        None => "—".to_string(),
    };
    let created = format!(
        "{} · {}",
        clock_label(run.created_at),
        duration_label(model.run_elapsed_ms(run))
    );

    button(
        row![
            mono(run.id.as_str(), 12.0, INK).width(Length::FillPortion(2)),
            stamp(run.state.stamp_label(), tone_for_run(run.state)).width(Length::FillPortion(2)),
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

/// One run: its facts, its VMs, its controls, and its serial.
fn run_pane<'a>(model: &'a Model, id: &str) -> Element<'a, Message> {
    let Some(run) = model.detail.as_ref().filter(|run| run.id == id) else {
        return serif("fetching the run…", 12.5, INK_2).into();
    };

    let mut vm_rows: Vec<Element<'a, Message>> =
        vec![rule::horizontal(1.0).style(theme::rule).into()];
    for vm in &run.vms {
        vm_rows.push(vm_row(vm));
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
            ("created", clock_label(run.created_at)),
            ("elapsed", duration_label(model.run_elapsed_ms(run))),
            (
                "ended",
                match run.ended_at {
                    Some(ended) => clock_label(ended),
                    None => "—".to_string(),
                },
            ),
        ]),
        controls(model, run),
        Column::with_children(vm_rows).spacing(4.0).into(),
        serial_pane(model, run),
    ])
    .spacing(12.0)
    .height(Length::Fill)
    .into()
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

/// One VM's line.
fn vm_row(vm: &Vm) -> Element<'_, Message> {
    let serial = match vm.serial.as_deref() {
        Some(serial) => format!("serial · {} bytes", serial.len()),
        None => "serial · in the run pane".to_string(),
    };
    let cid = match vm.guest_cid {
        Some(cid) => format!("cid {cid}"),
        None => "no vsock".to_string(),
    };
    row![
        mono(format!("vm {:02}", vm.id), 11.0, INK).width(60),
        stamp(vm.state.stamp_label(), tone_for_vm(vm.state)).width(90),
        mono(cid, 10.5, INK_2).width(110),
        mono(serial, 10.5, INK_3),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center)
    .into()
}

/// The serial transcript: the last lines of the run's first VM, boxed.
fn serial_pane<'a>(model: &'a Model, run: &'a Run) -> Element<'a, Message> {
    let capture = run.serial();
    let (skipped, lines) = serial_tail(capture, SERIAL_TAIL_LINES);

    let mut body: Vec<Element<'a, Message>> = Vec::with_capacity(lines.len() + 2);
    if capture.is_empty() {
        body.push(serif("the guest has not written to its console yet", 12.0, INK_3).into());
    }
    if skipped > 0 {
        body.push(mono(format!("… {skipped} earlier line(s) not shown"), 10.0, INK_3).into());
    }
    for line in lines {
        body.push(mono(line, 11.0, INK).into());
    }

    let transcript = scrollable(Column::with_children(body).spacing(1.0).width(Length::Fill))
        .height(Length::Fill)
        .auto_scroll(model.follow);

    container(transcript)
        .style(theme::boxed)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(edge(10.0, 10.0, 10.0, 10.0))
        .into()
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

/// A stamped word, boxed in its tone.
fn stamp<'a>(word: &'static str, tone: Tone) -> Container<'a, Message> {
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
    }

    #[test]
    fn every_pane_builds_its_tree() {
        let mut model = Model::new();
        model.runs = vec![run()];
        model.detail = Some(run());
        model.polls = 3;

        for pane in [Pane::Fleet, Pane::Run("r1".to_string()), Pane::Spawn] {
            model.pane = pane;
            let _: Element<'_, Message> = view(&model);
        }
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
}
