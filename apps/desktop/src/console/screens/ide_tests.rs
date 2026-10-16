//! The IDE screen's tests: the reducer's messages, the hub answers, the terminals, the run and the layout.

use super::*;

fn ctx() -> Ctx {
    Ctx {
        client: Client::new("http://127.0.0.1:9", None, None),
    }
}

fn info(cid: u64, writable: bool, status: &str) -> WorkspaceInfo {
    WorkspaceInfo {
        id: format!("agent:{cid}"),
        kind: "agent".into(),
        owner: cid.to_string(),
        name: "scout".into(),
        writable,
        status: status.into(),
        os: None,
    }
}

/// A workstation `w3` named `studio`, on macOS, as the hub lists an online one.
fn station() -> WorkspaceInfo {
    WorkspaceInfo {
        id: "station:w3".into(),
        kind: "station".into(),
        owner: "w3".into(),
        name: "studio".into(),
        writable: true,
        status: "online".into(),
        os: Some("macos".into()),
    }
}

/// A screen on agent 7's workspace, with `a.txt` open at `h1`.
fn screen_with_a_tab() -> State {
    let mut st = State {
        loaded: true,
        workspaces: vec![info(7, true, "idle")],
        ..State::default()
    };
    dispatch(&mut st, Action::SetWorkspace(WorkspaceId::Agent(7)));
    let file = FileView {
        hash: Some("h1".into()),
        text: Some("one\n".into()),
    };
    let _ = update(
        &mut st,
        Msg::Opened {
            ws: "agent:7".into(),
            path: "a.txt".into(),
            res: Ok(file),
        },
        &ctx(),
    );
    st
}

/// The owner's edit: the buffer changes as the editor would change it.
fn type_into(st: &mut State, path: &str, text: &str) {
    st.tabs.get_mut(path).expect("open").buffer.edit(text);
}

#[test]
fn opening_a_file_sets_a_tab_and_makes_it_active() {
    let mut st = State {
        loaded: true,
        workspaces: vec![info(7, true, "idle")],
        ..State::default()
    };
    dispatch(&mut st, Action::SetWorkspace(WorkspaceId::Agent(7)));
    let file = FileView {
        hash: Some("h1".into()),
        text: Some("fn main() {}\n".into()),
    };
    let _ = update(
        &mut st,
        Msg::Opened {
            ws: "agent:7".into(),
            path: "src/main.rs".into(),
            res: Ok(file),
        },
        &ctx(),
    );
    assert!(st.tabs.contains_key("src/main.rs"));
    assert_eq!(st.ide.open, ["src/main.rs"]);
    assert_eq!(st.ide.active.as_deref(), Some("src/main.rs"));
    assert!(!st.tabs["src/main.rs"].buffer.dirty);
}

#[test]
fn a_file_answered_for_another_workspace_is_dropped() {
    let mut st = screen_with_a_tab();
    let file = FileView {
        hash: Some("x".into()),
        text: Some("late\n".into()),
    };
    let _ = update(
        &mut st,
        Msg::Opened {
            ws: "agent:9".into(),
            path: "b.txt".into(),
            res: Ok(file),
        },
        &ctx(),
    );
    assert!(!st.tabs.contains_key("b.txt"));
}

#[test]
fn a_409_marks_the_tab_and_keep_mine_adopts_the_hub_hash_so_the_next_save_wins() {
    let mut st = screen_with_a_tab();
    type_into(&mut st, "a.txt", "two\n");
    let refused = Msg::Saved {
        ws: "agent:7".into(),
        path: "a.txt".into(),
        sent: "two\n".into(),
        res: Err(SaveError::Conflict(Some("h2".into()))),
    };
    let _ = update(&mut st, refused, &ctx());
    let tab = &st.tabs["a.txt"];
    assert_eq!(
        tab.conflict,
        Some(Conflict {
            server_hash: Some("h2".into())
        })
    );
    assert!(tab.buffer.dirty, "a refused save keeps the owner's edits");
    assert_eq!(state_badge(&st, false).0, "conflict");

    let _ = update(&mut st, Msg::KeepMine("a.txt".into()), &ctx());
    let tab = &st.tabs["a.txt"];
    assert_eq!(tab.conflict, None);
    assert_eq!(
        tab.buffer.base_hash.as_deref(),
        Some("h2"),
        "the next save sends the hub's hash"
    );
    assert!(tab.buffer.dirty);
}

#[test]
fn a_saved_tab_is_clean_unless_the_owner_typed_during_the_save() {
    let mut tab = Tab::new("a.txt", Some("h1".into()), "one\n");
    tab.buffer.edit("two\n");
    tab.buffer.edit("three\n");
    tab.saved("two\n", "h2".into());
    assert!(
        tab.buffer.dirty,
        "the text moved on while the save was in flight"
    );
    assert_eq!(tab.buffer.base_hash.as_deref(), Some("h2"));
    tab.saved("three\n", "h3".into());
    assert!(!tab.buffer.dirty);
}

#[test]
fn a_remote_change_reloads_a_clean_tab_and_conflicts_an_edited_one() {
    let mut clean = Tab::new("a.txt", Some("h1".into()), "one\n");
    assert_eq!(clean.remote(Some("h1")), Remote::Same);
    assert_eq!(clean.remote(Some("h9")), Remote::Reload);

    let mut edited = Tab::new("a.txt", Some("h1".into()), "one\n");
    edited.buffer.edit("mine\n");
    assert_eq!(edited.remote(Some("h10")), Remote::Conflict);
    assert_eq!(
        edited.conflict,
        Some(Conflict {
            server_hash: Some("h10".into())
        })
    );
    assert_eq!(edited.buffer.text, "mine\n", "the owner's text stays");
    assert_eq!(
        edited.remote(Some("h1")),
        Remote::Same,
        "the hub moved back: the conflict goes"
    );
    assert_eq!(edited.conflict, None);
}

#[test]
fn a_remote_event_for_a_clean_open_file_reloads_it() {
    let mut st = screen_with_a_tab();
    let fresh = FileView {
        hash: Some("h9".into()),
        text: Some("nine\n".into()),
    };
    let _ = update(
        &mut st,
        Msg::Remote {
            ws: "agent:7".into(),
            path: "a.txt".into(),
            res: Ok(fresh),
        },
        &ctx(),
    );
    let tab = &st.tabs["a.txt"];
    assert_eq!(tab.buffer.text, "nine\n");
    assert_eq!(tab.buffer.base_hash.as_deref(), Some("h9"));
}

#[test]
fn switching_workspace_is_refused_while_an_edit_is_unsaved() {
    let mut st = screen_with_a_tab();
    st.workspaces.push(info(8, true, "idle"));
    type_into(&mut st, "a.txt", "two\n");
    let _ = update(&mut st, Msg::Pick(choice(&info(8, true, "idle"))), &ctx());
    assert_eq!(st.ide.workspace, Some(WorkspaceId::Agent(7)));
    assert!(st.notice.is_some());
}

#[test]
fn the_hub_route_percent_encodes_the_workspace_id() {
    let repo = WorkspaceId::Repo {
        name: "plaza-web".into(),
        git_ref: "feat/x".into(),
    };
    assert_eq!(
        route(&repo, "/file"),
        "/v1/ide/workspaces/repo%3Aplaza-web%40feat%2Fx/file"
    );
    assert_eq!(enc("a b/c"), "a%20b%2Fc");
    assert_eq!(enc("keep-._~"), "keep-._~");
}

#[test]
fn checkpoints_come_as_a_bare_array_or_under_checkpoints() {
    let bare = json!([{"n": 1, "label": "start", "files": 2}]);
    let wrapped = json!({"checkpoints": [{"n": 1, "label": "start", "files": 2}]});
    assert_eq!(
        parse_checkpoints(bare).expect("bare"),
        parse_checkpoints(wrapped).expect("wrapped")
    );
    assert!(parse_checkpoints(json!({})).expect("empty").is_empty());
}

#[test]
fn a_workstation_has_its_files_editor_and_terminal_and_no_diff_chat_or_project_tabs() {
    let ws = WorkspaceId::Station("w3".into());
    assert!(!has_diff(Some(&ws)), "a workstation has no diff");
    assert!(
        has_shell(Some(&ws)),
        "a workstation has a shell: its terminal and runs"
    );
    assert_eq!(
        centre_views(Some(&ws)),
        vec![Centre::Terminal, Centre::Editor]
    );
    assert_eq!(right_tabs(Some(&ws)), vec![RightTab::Files]);
    // The other kinds keep what they had.
    let agent = WorkspaceId::Agent(7);
    let repo = WorkspaceId::Repo {
        name: "plaza".into(),
        git_ref: "main".into(),
    };
    assert!(has_diff(Some(&agent)) && has_diff(Some(&repo)) && has_diff(None));
    assert!(has_shell(Some(&agent)) && !has_shell(Some(&repo)) && !has_shell(None));
    assert_eq!(centre_views(Some(&agent)).len(), 4);
    assert_eq!(centre_views(Some(&repo)).len(), 4);
    assert_eq!(right_tabs(Some(&agent)).len(), 5);
    assert_eq!(right_tabs(Some(&repo)).len(), 5);
}

#[test]
fn selecting_a_workstation_moves_the_centre_and_the_panel_off_the_views_it_lacks() {
    let mut st = State {
        loaded: true,
        workspaces: vec![station()],
        ..State::default()
    };
    st.centre = Centre::Chat;
    st.right = RightTab::Board;
    let _ = update(&mut st, Msg::Pick(choice(&station())), &ctx());
    assert_eq!(st.ide.workspace, Some(WorkspaceId::Station("w3".into())));
    assert_eq!(
        st.centre,
        Centre::Editor,
        "the chat gives way to the editor"
    );
    assert_eq!(
        st.right,
        RightTab::Files,
        "the board gives way to the files"
    );
    let _ = update(&mut st, Msg::Centre(Centre::Chat), &ctx());
    let _ = update(&mut st, Msg::Centre(Centre::Diff), &ctx());
    assert_eq!(
        st.centre,
        Centre::Editor,
        "a workstation has no chat and no diff"
    );
    let _ = update(&mut st, Msg::Right(RightTab::Activity), &ctx());
    assert_eq!(st.right, RightTab::Files, "a workstation has no activity");
    let _ = update(&mut st, Msg::Centre(Centre::Terminal), &ctx());
    assert_eq!(st.centre, Centre::Terminal, "the terminal is its own view");
    assert!(st.terminals.is_some(), "and its shell's tabs are made");
}

#[test]
fn a_changed_filter_and_a_checkpoint_ask_the_hub_nothing_for_a_workstation() {
    let mut st = State {
        loaded: true,
        workspaces: vec![station()],
        ..State::default()
    };
    dispatch(
        &mut st,
        Action::SetWorkspace(WorkspaceId::Station("w3".into())),
    );
    let _ = update(&mut st, Msg::ChangedOnly, &ctx());
    assert!(!st.ide.filter_changed, "the changed-only filter is not set");
    let _ = changes_task(&mut st, &ctx());
    assert!(
        !st.changes_busy,
        "no changes are asked for: the hub has none for a workstation"
    );
    let _ = checkpoints_task(&st, &ctx());
    assert!(st.checkpoints.is_empty());
}

#[test]
fn the_terminal_takes_the_keys_while_it_has_focus() {
    let mut st = screen_with_a_tab();
    type_into(&mut st, "a.txt", "two\n");
    st.saved = false;
    let save_key = |st: &mut State| {
        let key = Key::Character("s".into());
        let _ = update(
            st,
            Msg::Shortcut {
                key,
                modifiers: Modifiers::COMMAND,
                status: Status::Ignored,
            },
            &ctx(),
        );
    };
    // The terminal has the keys: the command key's S (Ctrl+S, Cmd+S on macOS) is the shell's, so nothing is saved.
    st.terminal_hover = true;
    let _ = update(&mut st, Msg::Pressed, &ctx());
    assert!(st.terminal_focus);
    save_key(&mut st);
    assert_eq!(
        st.notice, None,
        "the terminal's command+S is not the IDE's save"
    );
    // A press elsewhere (the editor) gives the keys back: the command key's S is the IDE's save again.
    st.terminal_hover = false;
    let _ = update(&mut st, Msg::Pressed, &ctx());
    assert!(!st.terminal_focus);
    assert!(matches!(
        shortcut(
            &Key::Character("s".into()),
            Modifiers::COMMAND,
            Status::Ignored
        ),
        Some(Msg::Save)
    ));
}

#[test]
fn a_repo_workspace_makes_no_terminal() {
    let mut st = State {
        loaded: true,
        ..State::default()
    };
    dispatch(
        &mut st,
        Action::SetWorkspace(WorkspaceId::Repo {
            name: "plaza".into(),
            git_ref: "main".into(),
        }),
    );
    let _ = update(&mut st, Msg::Centre(Centre::Terminal), &ctx());
    let _ = update(&mut st, Msg::ToggleDrawer, &ctx());
    assert!(st.terminals.is_none(), "repo workspaces run no shell");
}

#[test]
fn a_tree_listing_for_the_current_workspace_fills_the_tree() {
    let mut st = screen_with_a_tab();
    let page = TreePage {
        entries: vec![apz_ide::Entry {
            path: "a.txt".into(),
            kind: EntryKind::File,
            size: 4,
            hash: Some("h1".into()),
            binary: false,
        }],
        next: None,
    };
    let _ = update(
        &mut st,
        Msg::Dir {
            ws: "agent:7".into(),
            dir: String::new(),
            first: true,
            res: Ok(page),
        },
        &ctx(),
    );
    assert!(st.tree.loaded.contains(""));
    assert_eq!(st.tree.entries.len(), 1);
}

#[test]
fn a_target_names_the_workspace_and_its_flags() {
    assert_eq!(target("agent:7", false, None), "agent:7");
    assert_eq!(
        target("repo:plaza@main", true, None),
        "repo:plaza@main?review"
    );
    let linked = target("repo:plaza@feat/x", false, Some("src/a b.rs"));
    assert_eq!(linked, "repo:plaza@feat/x?open=src%2Fa%20b.rs");
    assert_eq!(
        Target::parse(&linked),
        Target {
            workspace: "repo:plaza@feat/x".into(),
            review: false,
            open: Some("src/a b.rs".into()),
        }
    );
    let both = Target::parse(&target("agent:7", true, Some("x.rs")));
    assert!(both.review);
    assert_eq!(both.open.as_deref(), Some("x.rs"));
    assert_eq!(
        Target::parse("agent:7"),
        Target {
            workspace: "agent:7".into(),
            ..Target::default()
        }
    );
    assert_eq!(dec("100%"), "100%", "a stray percent sign stays as it is");
}

/// The terminals on screen: each tab's slot, shell, offset and identity.
fn tabs_held(st: &State) -> Vec<(usize, Option<String>, u64, u64)> {
    let Some(tabs) = st.terminals.as_ref() else {
        return Vec::new();
    };
    (0..tabs.len())
        .filter_map(|index| tabs.get(index))
        .map(|tab| {
            (
                tab.slot(),
                tab.term().map(str::to_string),
                tab.offset(),
                tab.id(),
            )
        })
        .collect()
}

/// A tab's message from the socket, by its slot.
fn from_tab(slot: usize, link: ide_terminal::Link) -> Msg {
    Msg::Terminals(TabsMsg::Tab(slot, ide_terminal::Msg::Link(link)))
}

#[test]
fn leaving_and_entering_makes_the_terminals_once_at_their_offsets() {
    let mut st = screen_with_a_tab();
    let _ = update(&mut st, Msg::Centre(Centre::Terminal), &ctx());
    let _ = update(&mut st, Msg::Terminals(TabsMsg::Add), &ctx());
    let _ = update(
        &mut st,
        from_tab(0, ide_terminal::Link::Opened("term-a".into())),
        &ctx(),
    );
    let _ = update(
        &mut st,
        from_tab(0, ide_terminal::Link::Output(b"hello".to_vec(), 5)),
        &ctx(),
    );
    let _ = update(
        &mut st,
        from_tab(1, ide_terminal::Link::Opened("term-b".into())),
        &ctx(),
    );
    let _ = update(
        &mut st,
        from_tab(1, ide_terminal::Link::Output(b"world!".to_vec(), 6)),
        &ctx(),
    );
    let shown = tabs_held(&st);
    let kept = |held: &[(usize, Option<String>, u64, u64)]| {
        held.iter()
            .map(|t| (t.0, t.1.clone(), t.2))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        kept(&shown),
        vec![
            (0, Some("term-a".to_string()), 5),
            (1, Some("term-b".to_string()), 6)
        ]
    );

    left(&mut st);
    assert!(st.terminals.is_none(), "the tabs leave the screen");
    assert!(st.parked.is_some(), "the shells and their offsets are kept");
    // A message from a socket that is still ending takes no part while the IDE is away.
    let _ = update(
        &mut st,
        from_tab(0, ide_terminal::Link::Output(b"late".to_vec(), 9)),
        &ctx(),
    );
    assert!(st.terminals.is_none());

    let _ = entered(&mut st, &ctx());
    let back = tabs_held(&st);
    assert_eq!(
        kept(&back),
        kept(&shown),
        "the same shells at the same offsets"
    );
    assert!(
        back.iter().zip(&shown).all(|(new, old)| new.3 != old.3),
        "the tabs are made again, once"
    );
    assert!(st.parked.is_none(), "what was kept is used");

    // Entered again with no leave in between, and loaded again: nothing is made again.
    let _ = entered(&mut st, &ctx());
    let _ = load(&mut st, &ctx());
    assert_eq!(tabs_held(&st), back);
}

#[test]
fn a_run_is_not_read_on_while_the_ide_is_away_and_is_polled_again_on_entry() {
    let mut st = screen_with_a_tab();
    st.panels.run = ide_panels::Run {
        phase: ide_panels::Phase::Running,
        run: Some("run-1".into()),
        ..ide_panels::Run::default()
    };
    st.poll_inflight = true;
    left(&mut st);
    let answer = Msg::Panels(ide_panels::Msg::Polled {
        ws: "agent:7".into(),
        res: Ok(ide_panels::Poll {
            output: "late\n".into(),
            next: 5,
            done: false,
            exit_code: None,
            timed_out: false,
        }),
    });
    let _ = update(&mut st, answer, &ctx());
    assert!(
        !st.poll_inflight,
        "the answer ends the poll while the IDE is away"
    );
    assert!(
        st.panels.run.output.is_empty(),
        "the run is not read on while the IDE is away"
    );
    assert_eq!(st.panels.run.offset, 0);
    let _ = entered(&mut st, &ctx());
    assert!(
        st.poll_inflight,
        "entering asks the run for its output again"
    );
    assert!(
        st.panels.run.output.is_empty(),
        "the output comes with that poll's answer"
    );
}
