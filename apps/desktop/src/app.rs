//! The application itself: boot, update, the runtime that turns an effect
//! into daemon calls, and the poll timer.
//!
//! Kept out of the binary so the same wiring can be driven headlessly (the
//! render test in `tests/render.rs` runs this exact application).

use crate::api::Client;
use crate::cli::Config;
use crate::export::{self, ExportCall};
use crate::model::{ConsoleCall, Effect, KeyAction, Message, Model, title};
use crate::{epoch_ms, theme, view};

use iced::keyboard::{self, Key};
use iced::{Application, Program, Subscription, Task};

/// The dashboard's program, wired to `config`'s daemon.
pub fn application(
    config: Config,
) -> Application<impl Program<State = Model, Message = Message, Theme = iced::Theme>> {
    let client = Client::new(config.endpoint.clone()).with_timeout(config.timeout);
    let model = Model::at_cadence(config.endpoint, config.refresh_ms);

    // The first poll is the first tick: the reducer schedules it and counts
    // it, and the timer takes over from there.
    let boot_client = client.clone();
    let boot = move || {
        let mut model = model.clone();
        let effect = model.boot(epoch_ms());
        (model, dispatch(effect, &boot_client))
    };
    let update = move |model: &mut Model, message: Message| {
        let effect = model.apply(message);
        dispatch(effect, &client)
    };

    iced::application(boot, update, view::view)
        .title(title)
        .theme(|_: &Model| theme::theme())
        .subscription(subscriptions)
        .window_size([1280.0, 860.0])
        .antialiasing(true)
}

/// Runs the dashboard until the window closes.
pub fn run(config: Config) -> iced::Result {
    application(config).run()
}

/// Everything the runtime listens for: the poll timer at the cadence the
/// register was given, and the keys it binds.
fn subscriptions(model: &Model) -> Subscription<Message> {
    Subscription::batch([bell(model), keys()])
}

/// The poll timer, carrying the wall clock: a tick every `refresh_ms`, which
/// is the finest granularity the schedule has, since the model decides whether
/// a tick becomes a call.
fn bell(model: &Model) -> Subscription<Message> {
    iced::time::every(std::time::Duration::from_millis(model.refresh_ms))
        .map(|_| Message::Tick { at_ms: epoch_ms() })
}

/// The register's keys. A focused text box captures its own, so typing a
/// console line never trips them.
fn keys() -> Subscription<Message> {
    keyboard::listen().filter_map(|event| match event {
        keyboard::Event::KeyPressed {
            key,
            modifiers,
            repeat: false,
            ..
        } if !modifiers.command() && !modifiers.control() && !modifiers.alt() => {
            shortcut(&key).map(Message::Key)
        }
        _ => None,
    })
}

/// The action a key names, if the register binds it: `r`, `m`, `f` in either
/// case, `?`, Escape, and the console's arrows.
pub fn shortcut(key: &Key) -> Option<KeyAction> {
    match key.as_ref() {
        Key::Character(character) => match character.to_ascii_lowercase().as_str() {
            "r" => Some(KeyAction::Register),
            "m" => Some(KeyAction::Machine),
            "f" => Some(KeyAction::Map),
            "?" => Some(KeyAction::Help),
            _ => None,
        },
        Key::Named(key) => match key {
            keyboard::key::Named::Escape => Some(KeyAction::Back),
            keyboard::key::Named::ArrowUp => Some(KeyAction::HistoryPrev),
            keyboard::key::Named::ArrowDown => Some(KeyAction::HistoryNext),
            _ => None,
        },
        Key::Unidentified => None,
    }
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
    if effect.poll_machine {
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.machine();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Machine { at_ms, result },
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
    for id in effect.fetch_map {
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
    if let Some(ConsoleCall { run, vm, data }) = effect.input {
        let client = client.clone();
        let written = run.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.input(&run, vm, &data);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::Input {
                at_ms,
                id: written.clone(),
                vm,
                result,
            },
        ));
    }
    if let Some(ExportCall {
        run,
        vm,
        path,
        contents,
    }) = effect.export
    {
        // The register's one local side effect: a transcript written to the
        // path in the box, with the disk's own words if it cannot land.
        let written = path.clone();
        tasks.push(Task::perform(
            async move {
                (
                    epoch_ms(),
                    export::write_transcript(&path, &contents),
                )
            },
            move |(at_ms, result)| Message::Exported {
                at_ms,
                run: run.clone(),
                vm,
                path: written.clone(),
                result,
            },
        ));
    }
    Task::batch(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_binding_keys_name_the_panes_and_the_console_arrows() {
        for (key, action) in [
            (Key::Character("r".into()), KeyAction::Register),
            (Key::Character("m".into()), KeyAction::Machine),
            (Key::Character("f".into()), KeyAction::Map),
            // A shifted letter is the same logical key in caps.
            (Key::Character("R".into()), KeyAction::Register),
            (Key::Character("M".into()), KeyAction::Machine),
            (Key::Character("F".into()), KeyAction::Map),
            (Key::Named(keyboard::key::Named::Escape), KeyAction::Back),
            (Key::Character("?".into()), KeyAction::Help),
            (Key::Named(keyboard::key::Named::ArrowUp), KeyAction::HistoryPrev),
            (
                Key::Named(keyboard::key::Named::ArrowDown),
                KeyAction::HistoryNext,
            ),
        ] {
            assert_eq!(shortcut(&key), Some(action), "{key:?}");
        }
    }

    #[test]
    fn a_key_the_register_does_not_bind_is_not_an_action() {
        for key in [
            Key::Character("x".into()),
            Key::Character("".into()),
            Key::Named(keyboard::key::Named::Enter),
            Key::Named(keyboard::key::Named::Tab),
            Key::Unidentified,
        ] {
            assert_eq!(shortcut(&key), None, "{key:?}");
        }
    }
}
