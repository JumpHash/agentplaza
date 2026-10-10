//! The application itself: boot, update, the runtime that turns an effect
//! into daemon calls, and the poll timer.
//!
//! Kept out of the binary so the same wiring can be driven headlessly (the
//! render test in `tests/render.rs` runs this exact application).

use crate::api::Client;
use crate::cli::Config;
use crate::model::{Effect, Message, Model};
use crate::{TICK, epoch_ms, theme, view};

use iced::{Application, Program, Subscription, Task};

/// The dashboard's program, wired to `config`'s daemon.
pub fn application(
    config: Config,
) -> Application<impl Program<State = Model, Message = Message, Theme = iced::Theme>> {
    let client = Client::new(config.endpoint.clone()).with_timeout(config.timeout);
    let model = Model::at(config.endpoint);

    // The first poll is the first tick: the reducer schedules it and counts
    // it, and the 2 s timer takes over from there.
    let boot_client = client.clone();
    let boot = move || {
        let mut model = model.clone();
        let effect = model.apply(Message::Tick { at_ms: epoch_ms() });
        (model, dispatch(effect, &boot_client))
    };
    let update = move |model: &mut Model, message: Message| {
        let effect = model.apply(message);
        dispatch(effect, &client)
    };

    iced::application(boot, update, view::view)
        .title("AgentPlaza register — fleet")
        .theme(|_: &Model| theme::theme())
        .subscription(bell)
        .window_size([1280.0, 860.0])
        .antialiasing(true)
}

/// Runs the dashboard until the window closes.
pub fn run(config: Config) -> iced::Result {
    application(config).run()
}

/// The poll timer: a tick every two seconds, carrying the wall clock.
fn bell(_model: &Model) -> Subscription<Message> {
    iced::time::every(TICK).map(|_| Message::Tick { at_ms: epoch_ms() })
}

/// Runs one effect's calls concurrently and turns their answers into messages.
fn dispatch(effect: Effect, client: &Client) -> Task<Message> {
    let mut tasks: Vec<Task<Message>> = Vec::new();
    if effect.poll_list {
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.runs();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Runs { at_ms, result },
        ));
    }
    if let Some(id) = effect.fetch_run {
        let client = client.clone();
        let requested = id.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.run(&id);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::Run {
                at_ms,
                id: requested.clone(),
                result,
            },
        ));
    }
    if let Some(id) = effect.kill {
        let client = client.clone();
        let killed = id.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.kill(&id);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::Killed {
                at_ms,
                id: killed.clone(),
                result,
            },
        ));
    }
    if let Some(create) = effect.spawn {
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.spawn(&create);
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Spawned { at_ms, result },
        ));
    }
    Task::batch(tasks)
}
