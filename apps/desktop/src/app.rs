//! The application itself: boot, update, the runtime that turns an effect
//! into daemon calls, and the poll timer.
//!
//! Kept out of the binary so the same wiring can be driven headlessly (the
//! render test in `tests/render.rs` runs this exact application).

use crate::api::{Client, Endpoint};
use crate::cli::Config;
use crate::events::{self, Event};
use crate::export::{self, CsvCall, ExportCall};
use crate::model::{ConsoleCall, Effect, KeyAction, Message, Model, title};
use crate::{epoch_ms, theme, view};

use std::pin::Pin;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use iced::keyboard::{self, Key};
use iced::{Application, Program, Subscription, Task};

/// The dashboard's program, wired to `config`'s daemon.
pub fn application(
    config: Config,
) -> Application<impl Program<State = Model, Message = Message, Theme = iced::Theme>> {
    let client = Client::new(config.endpoint.clone()).with_timeout(config.timeout);
    // The hub is a second process on a second loopback port, read with the
    // same client: `None` when no `--hub-url` named one.
    let hub = config
        .hub
        .clone()
        .map(|endpoint| Client::new(endpoint).with_timeout(config.timeout));
    // The hub's admin surface is yet another: the one the roster's operator
    // controls drive (`POST /tell`, `POST /stop`). `None` when no
    // `--hub-admin-url` named one, which leaves a read-only roster.
    let admin = config
        .hub_admin
        .clone()
        .map(|endpoint| Client::new(endpoint).with_timeout(config.timeout));
    let model = Model::at_cadence(config.endpoint, config.refresh_ms)
        .with_hub(config.hub)
        .with_hub_admin(config.hub_admin);

    // The first poll is the first tick: the reducer schedules it and counts
    // it, and the timer takes over from there.
    let boot_client = client.clone();
    let boot_hub = hub.clone();
    let boot_admin = admin.clone();
    let boot = move || {
        let mut model = model.clone();
        let effect = model.boot(epoch_ms());
        (
            model,
            dispatch(effect, &boot_client, boot_hub.as_ref(), boot_admin.as_ref()),
        )
    };
    let update = move |model: &mut Model, message: Message| {
        let effect = model.apply(message);
        dispatch(effect, &client, hub.as_ref(), admin.as_ref())
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
/// register was given, the floor's own animation clock while a pair walks, the
/// keys it binds, and the daemon's event stream.
fn subscriptions(model: &Model) -> Subscription<Message> {
    Subscription::batch([bell(model), anim(model), keys(), live(&model.endpoint)])
}

/// The poll timer, carrying the wall clock: a tick every `refresh_ms`, which
/// is the finest granularity the schedule has, since the model decides whether
/// a tick becomes a call.
fn bell(model: &Model) -> Subscription<Message> {
    iced::time::every(std::time::Duration::from_millis(model.refresh_ms))
        .map(|_| Message::Tick { at_ms: epoch_ms() })
}

/// The floor's animation clock: a frame every [`crate::model::ANIM_MS`] while
/// the floor shows a pair still walking, and nothing at all otherwise.
///
/// This is the one place a redraw is asked for faster than the poll, and it is
/// bounded: it runs only while a walk is in progress — a window of at most
/// [`crate::model::WALK_MS`] — and it never reads the daemon. iced has no
/// reduced-motion signal, so the walk is short and always completes; there is
/// no way to ask it to stop, and none is claimed.
fn anim(model: &Model) -> Subscription<Message> {
    if !model.floor_animating() {
        return Subscription::none();
    }
    iced::time::every(std::time::Duration::from_millis(crate::model::ANIM_MS))
        .map(|_| Message::Anim { at_ms: epoch_ms() })
}

/// `GET /v1/events`, as a subscription.
///
/// The endpoint is the subscription's identity, so the same daemon keeps the
/// same stream for the register's whole life. iced does **not** restart a
/// subscription whose stream has ended — its tracker keeps the recipe and
/// never spawns it again — so the reconnect lives inside the consumer
/// ([`events::follow`]) and this stream never completes on its own account.
fn live(endpoint: &Endpoint) -> Subscription<Message> {
    Subscription::run_with(endpoint.clone(), stream)
}

/// The stream the subscription runs: a thread following the daemon's events,
/// and the channel it hands them over on.
fn stream(endpoint: &Endpoint) -> EventStream {
    let (sender, receiver) = std::sync::mpsc::channel();
    let waker: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));

    let wanted = endpoint.clone();
    let thread_waker = Arc::clone(&waker);
    std::thread::spawn(move || {
        events::follow(&wanted, &mut |event| {
            // The frame's clock: the daemon's frames carry no time of their
            // own, and when the console read it is what the register means by
            // "now".
            if sender.send((epoch_ms(), event)).is_err() {
                // The subscription went away with the receiver: end the
                // thread rather than keep a socket open for nobody.
                return false;
            }
            if let Some(waker) = thread_waker
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                waker.wake();
            }
            true
        });
    });

    EventStream { receiver, waker }
}

/// The bridge between the reader thread and the runtime: `std::sync::mpsc`
/// in, a [`Stream`](iced::futures::Stream) out.
///
/// The thread cannot wake an executor directly, so the poll stores the
/// runtime's waker and the thread takes it after every send. A message that
/// lands between the two looks in [`EventStream::poll_next`] is seen by the
/// second one, so nothing is lost and nothing is delivered twice.
struct EventStream {
    /// Where the reader thread's events arrive.
    receiver: Receiver<(u64, Event)>,
    /// The runtime's waker, kept only while the stream is waiting.
    waker: Arc<Mutex<Option<Waker>>>,
}

impl EventStream {
    /// The next message, if the thread has sent one.
    fn take(&self) -> Option<Option<Message>> {
        match self.receiver.try_recv() {
            Ok((at_ms, event)) => Some(Some(Message::Event { at_ms, event })),
            Err(TryRecvError::Disconnected) => Some(None),
            Err(TryRecvError::Empty) => None,
        }
    }
}

impl iced::futures::Stream for EventStream {
    type Item = Message;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Message>> {
        let this = self.get_mut();
        if let Some(message) = this.take() {
            return Poll::Ready(message);
        }
        // Register first, then look again: the thread's send happens before
        // its wake, so a message that lands in between is either caught by
        // the second look or wakes the waker just stored.
        *this
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(context.waker().clone());
        match this.take() {
            Some(message) => Poll::Ready(message),
            None => Poll::Pending,
        }
    }
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

/// The action a key names, if the register binds it: `r`, `m`, `f`, `w`, `l`,
/// `y`, `u` and `t` in either case, `?`, Escape, and the console's arrows.
pub fn shortcut(key: &Key) -> Option<KeyAction> {
    match key.as_ref() {
        Key::Character(character) => match character.to_ascii_lowercase().as_str() {
            "r" => Some(KeyAction::Register),
            "m" => Some(KeyAction::Machine),
            "f" => Some(KeyAction::Map),
            "w" => Some(KeyAction::Roster),
            "l" => Some(KeyAction::Ledger),
            "y" => Some(KeyAction::Treasury),
            "u" => Some(KeyAction::Market),
            "i" => Some(KeyAction::Floor),
            "j" => Some(KeyAction::Board),
            "k" => Some(KeyAction::Talk),
            "o" => Some(KeyAction::Council),
            "t" => Some(KeyAction::Telemetry),
            "?" => Some(KeyAction::Help),
            _ => None,
        },
        Key::Named(key) => match key {
            keyboard::key::Named::Escape => Some(KeyAction::Back),
            keyboard::key::Named::ArrowUp => Some(KeyAction::HistoryPrev),
            keyboard::key::Named::ArrowDown => Some(KeyAction::HistoryNext),
            keyboard::key::Named::Enter => Some(KeyAction::Inspect),
            _ => None,
        },
        Key::Unidentified => None,
    }
}

/// Runs one effect's calls concurrently and turns their answers into messages.
fn dispatch(
    effect: Effect,
    client: &Client,
    hub: Option<&Client>,
    admin: Option<&Client>,
) -> Task<Message> {
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
    if effect.poll_hub {
        // The hub's own client, and its own message: a hub that is down is
        // never mistaken for a daemon that is.
        if let Some(hub) = hub.cloned() {
            tasks.push(Task::perform(
                async move {
                    let result = hub.hub_status();
                    (epoch_ms(), result)
                },
                |(at_ms, result)| Message::Hub { at_ms, result },
            ));
        }
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
        target,
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
                    export::write_export(&path, &contents),
                )
            },
            move |(at_ms, result)| Message::Exported {
                at_ms,
                target,
                run: run.clone(),
                vm,
                path: written.clone(),
                result,
            },
        ));
    }
    if let Some(CsvCall { path, contents }) = effect.csv {
        // The same one write, over the session instead of a transcript.
        let written = path.clone();
        tasks.push(Task::perform(
            async move { (epoch_ms(), export::write_export(&path, &contents)) },
            move |(at_ms, result)| Message::CsvExported {
                at_ms,
                path: written.clone(),
                result,
            },
        ));
    }
    if effect.poll_tasks {
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.tasks();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Tasks { at_ms, result },
        ));
    }
    if effect.poll_treasury {
        // The pane's own read, both halves at once: the accounts with their
        // totals, and the journal. Its own message and its own schedule, so a
        // daemon that serves the register but not the treasury never marks the
        // register down.
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.treasury();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Treasury { at_ms, result },
        ));
    }
    if effect.poll_agora {
        // The market's own read: the whole board, every call with its bids and
        // its award. Its own message and its own schedule, so a daemon that
        // serves the register but not the agora never marks the register down.
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.agora();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Agora { at_ms, result },
        ));
    }
    if effect.poll_floor {
        // The floor's own read: the conversations and the board at once. Its
        // own message and its own schedule.
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.floor();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Floor { at_ms, result },
        ));
    }
    if effect.poll_board {
        // The board pane's own read: the public posts.
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.board();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Board { at_ms, result },
        ));
    }
    if effect.poll_talk {
        // The talk pane's own read: the conversation log, with the pair
        // summaries its filter walks. Its own message and its own schedule.
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.conversations();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Talk { at_ms, result },
        ));
    }
    if effect.poll_motions {
        // The council pane's own read: the seats' motions, newest first, each
        // with its votes and its tally. Its own message and its own schedule.
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.motions();
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::Motions { at_ms, result },
        ));
    }
    if let Some(cid) = effect.fetch_contacts {
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.contacts(cid);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::Contacts { at_ms, cid, result },
        ));
    }
    if let Some((a, b)) = effect.fetch_thread {
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.thread(a, b);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::Thread { at_ms, a, b, result },
        ));
    }
    if let Some(id) = effect.fetch_task {
        let client = client.clone();
        let requested = id.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.task(&id);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::Task {
                at_ms,
                id: requested.clone(),
                result,
            },
        ));
    }
    if let Some(create) = effect.create_task {
        let client = client.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.create_task(&create);
                (epoch_ms(), result)
            },
            |(at_ms, result)| Message::TaskCreated { at_ms, result },
        ));
    }
    if let Some(call) = effect.claim_task {
        let client = client.clone();
        let claimed = call.id.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.claim_task(&call.id, call.agent_cid);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::TaskClaimed {
                at_ms,
                id: claimed.clone(),
                cid: call.agent_cid,
                result,
            },
        ));
    }
    if let Some(call) = effect.finish_task {
        let client = client.clone();
        let finished = call.id.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.finish_task(&call.id, call.outcome, call.note.as_deref());
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::TaskFinished {
                at_ms,
                id: finished.clone(),
                outcome: call.outcome,
                result,
            },
        ));
    }
    if let Some(id) = effect.withdraw_task {
        let client = client.clone();
        let withdrawn = id.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.withdraw_task(&id);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::TaskWithdrawn {
                at_ms,
                id: withdrawn.clone(),
                result,
            },
        ));
    }
    if let Some(call) = effect.artifact {
        let client = client.clone();
        let requested = call.id.clone();
        let path = call.path.clone();
        tasks.push(Task::perform(
            async move {
                let result = client.task_artifact(&call.id, &call.path);
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::TaskArtifact {
                at_ms,
                id: requested.clone(),
                path: path.clone(),
                result,
            },
        ));
    }
    if let Some(call) = effect.tell_agent {
        // The hub's admin surface, its own client and its own message: a hub
        // that refuses a drive is never mistaken for a daemon that is down.
        if let Some(admin) = admin.cloned() {
            let cid = call.cid;
            let asked = call.name.clone();
            let reported = call.name.clone();
            tasks.push(Task::perform(
                async move {
                    let result = admin.hub_tell(cid, &asked, &call.args, call.timeout_ms);
                    (epoch_ms(), result)
                },
                move |(at_ms, result)| Message::Told {
                    at_ms,
                    cid,
                    name: reported.clone(),
                    result,
                },
            ));
        }
    }
    if let Some(cid) = effect.stop_agent {
        if let Some(admin) = admin.cloned() {
            tasks.push(Task::perform(
                async move {
                    let result = admin.hub_stop(cid);
                    (epoch_ms(), result)
                },
                move |(at_ms, result)| Message::Stopped { at_ms, cid, result },
            ));
        }
    }
    if let Some(call) = effect.decide_motion {
        // The owner's hand on the council, through the register's own client:
        // the decision is recorded on `noded`, the same daemon the motions are
        // read from — not the hub's admin surface.
        let client = client.clone();
        let id = call.id.clone();
        let reported = call.id.clone();
        let approve = call.approve;
        tasks.push(Task::perform(
            async move {
                let result = client.decide_motion(&id, approve, call.note.as_deref());
                (epoch_ms(), result)
            },
            move |(at_ms, result)| Message::Decided {
                at_ms,
                id: reported.clone(),
                approve,
                result,
            },
        ));
    }
    Task::batch(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Sender;

    use crate::api::RunState;
    use iced::futures::Stream;

    /// A waker that counts how many times the runtime was woken.
    #[derive(Default)]
    struct Wakes(AtomicUsize);

    impl iced::futures::task::ArcWake for Wakes {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A bridge with a channel the test holds, so the reader thread's own
    /// send-then-wake can be played out exactly.
    fn bridge() -> (Sender<(u64, Event)>, EventStream) {
        let (sender, receiver) = std::sync::mpsc::channel();
        (
            sender,
            EventStream {
                receiver,
                waker: Arc::new(Mutex::new(None)),
            },
        )
    }

    fn parked(bridge: &mut EventStream, waker: &Waker) -> Poll<Option<Message>> {
        let mut context = Context::from_waker(waker);
        Pin::new(bridge).poll_next(&mut context)
    }

    #[test]
    fn the_bridge_parks_with_the_runtimes_waker_and_reads_what_arrives() {
        let (sender, mut bridge) = bridge();
        let wakes = Arc::new(Wakes::default());
        let waker = iced::futures::task::waker(Arc::clone(&wakes));

        // Nothing has arrived: the stream parks, and it holds the waker.
        assert!(parked(&mut bridge, &waker).is_pending());
        assert!(
            bridge
                .waker
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some(),
            "the runtime's waker is kept while the stream waits"
        );

        // The reader thread's own sequence: send, then take the waker and
        // wake, because the runtime has no other way to hear about it.
        let event = Event::Run {
            id: "r1".to_string(),
            state: RunState::Starting,
        };
        sender.send((7, event.clone())).expect("the channel is open");
        let taken = bridge
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        taken.expect("the waker the runtime left").wake();
        assert_eq!(wakes.0.load(Ordering::SeqCst), 1, "the runtime is woken");

        assert_eq!(
            parked(&mut bridge, &waker),
            Poll::Ready(Some(Message::Event { at_ms: 7, event }))
        );
        assert!(
            parked(&mut bridge, &waker).is_pending(),
            "and then it parks again"
        );
    }

    #[test]
    fn a_message_that_arrives_before_the_waker_is_taken_is_still_delivered() {
        // The race the two looks in `poll_next` exist for: the thread's send
        // can land between the stream's look and the waker being stored, and
        // an event must never be lost to it.
        let (sender, mut bridge) = bridge();
        let wakes = Arc::new(Wakes::default());
        let waker = iced::futures::task::waker(Arc::clone(&wakes));

        let event = Event::Vm {
            run: "r1".to_string(),
            vm: 2,
            state: crate::api::VmState::Exited,
        };
        sender.send((9, event.clone())).expect("the channel is open");

        assert_eq!(
            parked(&mut bridge, &waker),
            Poll::Ready(Some(Message::Event { at_ms: 9, event })),
            "a message nobody woke the runtime for is still read"
        );
        assert_eq!(wakes.0.load(Ordering::SeqCst), 0, "no wake was needed");
    }

    #[test]
    fn the_stream_ends_when_its_reader_thread_goes_away() {
        let (sender, mut bridge) = bridge();
        let mut context = Context::from_waker(iced::futures::task::noop_waker_ref());
        assert!(Pin::new(&mut bridge).poll_next(&mut context).is_pending());

        // The thread ends when the subscription is gone, and the stream says
        // so rather than parking forever.
        drop(sender);
        assert_eq!(
            Pin::new(&mut bridge).poll_next(&mut context),
            Poll::Ready(None)
        );
    }

    #[test]
    fn the_binding_keys_name_the_panes_and_the_console_arrows() {
        for (key, action) in [
            (Key::Character("r".into()), KeyAction::Register),
            (Key::Character("m".into()), KeyAction::Machine),
            (Key::Character("f".into()), KeyAction::Map),
            (Key::Character("w".into()), KeyAction::Roster),
            (Key::Character("l".into()), KeyAction::Ledger),
            (Key::Character("y".into()), KeyAction::Treasury),
            (Key::Character("t".into()), KeyAction::Telemetry),
            (Key::Character("i".into()), KeyAction::Floor),
            (Key::Character("j".into()), KeyAction::Board),
            (Key::Character("k".into()), KeyAction::Talk),
            (Key::Character("o".into()), KeyAction::Council),
            // A shifted letter is the same logical key in caps.
            (Key::Character("R".into()), KeyAction::Register),
            (Key::Character("M".into()), KeyAction::Machine),
            (Key::Character("F".into()), KeyAction::Map),
            (Key::Character("W".into()), KeyAction::Roster),
            (Key::Character("L".into()), KeyAction::Ledger),
            (Key::Character("Y".into()), KeyAction::Treasury),
            (Key::Character("T".into()), KeyAction::Telemetry),
            (Key::Character("I".into()), KeyAction::Floor),
            (Key::Character("J".into()), KeyAction::Board),
            (Key::Character("K".into()), KeyAction::Talk),
            (Key::Character("O".into()), KeyAction::Council),
            (Key::Named(keyboard::key::Named::Escape), KeyAction::Back),
            (Key::Named(keyboard::key::Named::Enter), KeyAction::Inspect),
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
            Key::Named(keyboard::key::Named::Tab),
            Key::Unidentified,
        ] {
            assert_eq!(shortcut(&key), None, "{key:?}");
        }
    }
}
