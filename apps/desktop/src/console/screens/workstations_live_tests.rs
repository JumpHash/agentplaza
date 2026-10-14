use super::*;

fn frame_bytes(header: &str, image: &[u8]) -> Vec<u8> {
    let mut out = (header.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(image);
    out
}

#[test]
fn a_frame_header_parses_into_its_size_sequence_and_image() {
    let bytes = frame_bytes(
        r#"{"w":1920,"h":1080,"format":"jpeg","ts":5,"seq":7}"#,
        &[1, 2, 3],
    );
    let frame = parse_frame(&bytes).expect("a whole frame parses");
    assert_eq!((frame.w, frame.h, frame.seq), (1920, 1080, 7));
    assert_eq!(frame.image, vec![1, 2, 3]);
}

#[test]
fn a_frame_that_is_not_whole_or_not_a_picture_is_refused() {
    assert!(parse_frame(&[0, 0]).is_err(), "shorter than the length");
    assert!(
        parse_frame(&[0, 0, 0, 40, b'{']).is_err(),
        "header runs past the end"
    );
    assert!(parse_frame(&frame_bytes("not json", &[1])).is_err());
    assert!(
        parse_frame(&frame_bytes(r#"{"w":0,"h":1,"format":"png"}"#, &[1])).is_err(),
        "no width"
    );
    assert!(
        parse_frame(&frame_bytes(r#"{"w":1,"h":1,"format":"gif"}"#, &[1])).is_err(),
        "a format the hub never sends"
    );
    assert!(
        parse_frame(&frame_bytes(r#"{"w":1,"h":1,"format":"png"}"#, &[])).is_err(),
        "no image"
    );
}

#[test]
fn text_messages_of_the_picture_and_the_takeover_are_read() {
    assert_eq!(
        stream_text(r#"{"t":"started","session":3}"#),
        Some(StreamEvent::Started)
    );
    assert_eq!(
        stream_text(r#"{"t":"ended","reason":"stopped"}"#),
        Some(StreamEvent::Ended("stopped".into()))
    );
    assert_eq!(stream_text(r#"{"t":"pong"}"#), None);
    assert_eq!(
        control_text(r#"{"t":"taken","session":3,"policy":"release_lease"}"#),
        Some(ControlEvent::Taken("release_lease".into()))
    );
    assert_eq!(
        control_text(r#"{"t":"given_back"}"#),
        Some(ControlEvent::GivenBack)
    );
    assert_eq!(
        control_text(r#"{"t":"error","error":"no session"}"#),
        Some(ControlEvent::Refused("no session".into()))
    );
    assert_eq!(
        control_text(r#"{"t":"ended"}"#),
        Some(ControlEvent::Closed("The session ended.".into()))
    );
    assert_eq!(
        policy_of("keep_working_pointer_paused"),
        Takeover::KeepWorkingPointerPaused
    );
    assert_eq!(policy_of("nonsense"), Takeover::AgentWaits);
}

fn mods(ctrl: bool, shift: bool, alt: bool, logo: bool) -> Mods {
    Mods {
        ctrl,
        shift,
        alt,
        logo,
    }
}

fn press(key: Key, text: Option<&str>, modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text: text.map(Into::into),
        repeat: false,
    })
}

#[test]
fn the_owners_keys_reach_the_station_unless_a_field_took_them() {
    let id = iced::window::Id::unique();
    let chord = keys(
        press(Key::Character("s".into()), None, keyboard::Modifiers::CTRL),
        event::Status::Ignored,
        id,
    );
    assert!(
        matches!(&chord, Some(Msg::Key(k)) if k.key == KeyName::Char('s') && k.mods.ctrl),
        "ctrl+s is the key s with ctrl: {chord:?}"
    );
    let typed = keys(
        press(
            Key::Character("a".into()),
            Some("a"),
            keyboard::Modifiers::empty(),
        ),
        event::Status::Ignored,
        id,
    );
    assert!(
        matches!(&typed, Some(Msg::Key(k)) if k.text.as_deref() == Some("a") && k.mods == Mods::default())
    );
    let named = keys(
        press(
            Key::Named(Named::ArrowLeft),
            None,
            keyboard::Modifiers::SHIFT,
        ),
        event::Status::Ignored,
        id,
    );
    assert!(matches!(&named, Some(Msg::Key(k)) if k.key == KeyName::Named("left") && k.mods.shift));
    let taken = keys(
        press(Key::Named(Named::Enter), None, keyboard::Modifiers::empty()),
        event::Status::Captured,
        id,
    );
    assert!(taken.is_none(), "a text field that took the key keeps it");
    let released = Event::Keyboard(keyboard::Event::KeyReleased {
        key: Key::Named(Named::Enter),
        modified_key: Key::Named(Named::Enter),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::empty(),
    });
    assert!(
        keys(released, event::Status::Ignored, id).is_none(),
        "only presses are sent"
    );
}

#[test]
fn typed_text_is_a_type_action_and_a_named_key_is_a_key_action() {
    let none = mods(false, false, false, false);
    assert_eq!(
        key_input(&KeyName::Char('a'), Some("a"), none),
        Some(json!({"action": "type", "text": "a"}))
    );
    assert_eq!(
        key_input(
            &KeyName::Char('A'),
            Some("A"),
            mods(false, true, false, false)
        ),
        Some(json!({"action": "type", "text": "A"})),
        "shift with a letter is the letter it typed"
    );
    assert_eq!(
        key_input(&KeyName::Named("enter"), None, none),
        Some(json!({"action": "key", "combo": "enter"}))
    );
    assert_eq!(
        key_input(&KeyName::Named("f5"), None, none),
        Some(json!({"action": "key", "combo": "f5"}))
    );
}

#[test]
fn chords_name_their_modifiers_in_the_daemon_order() {
    assert_eq!(
        key_input(&KeyName::Char('S'), None, mods(true, true, false, false)),
        Some(json!({"action": "key", "combo": "ctrl+shift+s"}))
    );
    assert_eq!(
        key_input(
            &KeyName::Char('c'),
            Some("\u{3}"),
            mods(true, false, false, false)
        ),
        Some(json!({"action": "key", "combo": "ctrl+c"})),
        "a control character is never typed"
    );
    assert_eq!(
        key_input(
            &KeyName::Named("tab"),
            None,
            mods(false, true, false, false)
        ),
        Some(json!({"action": "key", "combo": "shift+tab"}))
    );
    assert_eq!(
        key_input(&KeyName::Char('q'), None, mods(false, false, false, true)),
        Some(json!({"action": "key", "combo": "cmd+q"}))
    );
    assert_eq!(
        key_input(
            &KeyName::Char('é'),
            Some("é"),
            mods(false, false, true, false)
        ),
        None,
        "a character the daemon cannot name is not sent in a chord"
    );
}

#[test]
fn a_wheel_turn_up_scrolls_up_and_a_pixel_delta_counts_notches() {
    assert_eq!(
        wheel_notches(0.0, 1.0, false),
        (0, -1),
        "iced's up is the hub's dy < 0"
    );
    assert_eq!(
        wheel_notches(0.0, -2.0, false),
        (0, 2),
        "a wheel turned down is dy > 0"
    );
    assert_eq!(wheel_notches(3.0, 0.0, false), (3, 0));
    assert_eq!(
        wheel_notches(0.0, -100.0, true),
        (0, 3),
        "100 pixels is 2.5 notches, rounded"
    );
    assert_eq!(
        wheel_notches(0.0, -3.0, true),
        (0, 1),
        "any movement is at least one notch"
    );
    assert_eq!(wheel_notches(0.0, 0.0, true), (0, 0));
}

#[test]
fn a_frame_is_fitted_and_centred_and_the_pointer_maps_into_its_pixels() {
    let (dw, dh, left, top) = fit((1000.0, 1000.0), (1920, 1080));
    assert!(
        (dw - 1000.0).abs() < 1e-3 && (dh - 562.5).abs() < 1e-3,
        "{dw}x{dh}"
    );
    assert!(left.abs() < 1e-3);
    assert!((top - 218.75).abs() < 1e-3);
    // The centre of the box is the centre of the frame.
    assert_eq!(
        frame_point(Point::new(500.0, 500.0), (1000.0, 1000.0), (1920, 1080)),
        (960, 540)
    );
    // The margin above the picture clamps to its top edge; the far corner is the last pixel.
    assert_eq!(
        frame_point(Point::new(0.0, 0.0), (1000.0, 1000.0), (1920, 1080)),
        (0, 0)
    );
    assert_eq!(
        frame_point(Point::new(1000.0, 1000.0), (1000.0, 1000.0), (1920, 1080)),
        (1919, 1079)
    );
    // A box with no room shows nothing to point at.
    assert_eq!(
        frame_point(Point::new(5.0, 5.0), (0.0, 0.0), (1920, 1080)),
        (0, 0)
    );
}

#[test]
fn the_sessions_are_the_owner_screen_and_each_pixel_lease() {
    let station = Station {
        leases: vec![
            super::super::workstations::Lease {
                id: "l1".into(),
                kind: "pixel".into(),
                session: Some(9),
                ..Default::default()
            },
            super::super::workstations::Lease {
                id: "l2".into(),
                kind: "shared".into(),
                session: None,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut state = State::new("w1");
    assert_eq!(
        state.sessions.len(),
        1,
        "before the station is known: the owner's screen only"
    );
    state.sync(&station);
    assert_eq!(
        state.sessions.len(),
        2,
        "the owner's screen and the pixel session"
    );
    assert_eq!(state.sessions[1].value, "9");
    state.choice = Choice {
        value: "9".into(),
        label: String::new(),
    };
    state.sync(&station);
    assert_eq!(
        state.choice.label, "Agent session 9",
        "a picked session takes its listed label"
    );
    state.choice = Choice {
        value: "77".into(),
        label: String::new(),
    };
    state.sync(&station);
    assert_eq!(
        state.choice.value, "shared",
        "a session that is gone falls back to the owner's screen"
    );
}
