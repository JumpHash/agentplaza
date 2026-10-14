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
fn reload_after_a_conflict_takes_the_hub_text_and_is_clean() {
    let mut tab = Tab::new("a.txt", Some("h1".into()), "one\n");
    tab.buffer.edit("mine\n");
    tab.conflicted(Some("h2".into()));
    tab.reload("a.txt", Some("h2".into()), "theirs\n");
    assert_eq!(tab.conflict, None);
    assert_eq!(tab.buffer.text, "theirs\n");
    assert!(!tab.buffer.dirty);
    assert_eq!(tab.buffer.base_hash.as_deref(), Some("h2"));
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
fn escape_closes_quick_open_before_it_goes_back() {
    let mut st = screen_with_a_tab();
    st.quick = Some("a".into());
    let _ = update(&mut st, Msg::Escape, &ctx());
    assert_eq!(st.quick, None, "the first Escape closes quick open");
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
fn the_badge_reads_conflict_then_held_then_agent_then_saved_then_idle() {
    let mut st = screen_with_a_tab();
    assert_eq!(state_badge(&st, false).0, "idle");
    st.saved = true;
    assert_eq!(state_badge(&st, false).0, "saved");
    st.workspaces[0].status = "working".into();
    assert_eq!(state_badge(&st, false).0, "agent working");
    assert_eq!(
        state_badge(&st, true).0,
        "held",
        "a held plaza shows before the agent's work"
    );
    st.tabs
        .get_mut("a.txt")
        .expect("open")
        .conflicted(Some("h2".into()));
    assert_eq!(state_badge(&st, true).0, "conflict");
}

#[test]
fn the_picker_names_a_repo_by_its_branch_and_an_agent_by_its_status() {
    let repo = WorkspaceInfo {
        id: "repo:plaza@feat/x".into(),
        kind: "repo".into(),
        owner: String::new(),
        name: "plaza".into(),
        writable: true,
        status: String::new(),
    };
    assert_eq!(choice(&repo).label, "repo · plaza @feat/x");
    assert_eq!(choice(&info(7, true, "idle")).label, "agent · scout · idle");
}

#[test]
fn back_goes_to_the_screen_the_ide_was_opened_from_and_never_to_the_ide() {
    assert_eq!(back_target(None), Id::Overview, "no origin: the Overview");
    assert_eq!(back_target(Some(Id::Agents)), Id::Agents);
    assert_eq!(
        back_target(Some(Id::Ide(Some("agent:7".into())))),
        Id::Overview,
        "an IDE origin would loop"
    );
}

#[test]
fn escape_closes_the_search_before_it_goes_back() {
    let mut st = screen_with_a_tab();
    let _ = update(&mut st, Msg::ToggleSearch, &ctx());
    assert_eq!(
        st.right,
        RightTab::Files,
        "the search opens in the Files tab"
    );
    assert!(st.search.is_some());
    let _ = update(&mut st, Msg::Escape, &ctx());
    assert!(st.search.is_none(), "the first Escape closes the search");
}

#[test]
fn the_centre_tabs_pick_the_view_and_the_right_tabs_pick_their_panel() {
    let mut st = screen_with_a_tab();
    assert_eq!(
        st.centre,
        Centre::Chat,
        "the chat is the centre's first view"
    );
    let _ = update(&mut st, Msg::Centre(Centre::Terminal), &ctx());
    assert_eq!(st.centre, Centre::Terminal);
    assert!(
        st.terminals.is_some(),
        "the terminal view makes the shell's tabs"
    );
    let _ = update(&mut st, Msg::Right(RightTab::Activity), &ctx());
    assert_eq!(st.right, RightTab::Activity);
    assert!(st.layout.right_open, "a right tab opens the right panel");
}

#[test]
fn the_palette_opens_filters_moves_and_runs_its_item() {
    let mut st = screen_with_a_tab();
    let _ = update(&mut st, Msg::PaletteOpen, &ctx());
    assert!(st.palette.is_some());
    let _ = update(&mut st, Msg::PaletteQuery("diff".into()), &ctx());
    let first = ide_palette::filtered(&st)
        .into_iter()
        .next()
        .expect("the diff command matches");
    assert_eq!(first.label, "Show the diff");
    let _ = update(&mut st, Msg::PaletteRun(0), &ctx());
    assert!(st.palette.is_none(), "running an item closes the palette");
    assert_eq!(st.centre, Centre::Diff, "and the item's command runs");
}

#[test]
fn a_changed_file_shows_its_diff_in_the_centre_and_opening_the_editor_clears_it() {
    let mut st = screen_with_a_tab();
    st.diff_file = Some("a.txt".into());
    let _ = update(&mut st, Msg::Activate("a.txt".into()), &ctx());
    assert_eq!(st.diff_file, None, "a tab shown puts the editor back");
    let _ = update(&mut st, Msg::ShowDiff("a.txt".into()), &ctx());
    assert_eq!(st.diff_file.as_deref(), Some("a.txt"));
    let _ = update(&mut st, Msg::CloseDiff, &ctx());
    assert_eq!(st.diff_file, None);
}

#[test]
fn a_drag_on_an_edge_changes_its_width_and_a_double_press_resets_it() {
    let mut st = screen_with_a_tab();
    let _ = update(&mut st, Msg::DragStart(Edge::Left), &ctx());
    let _ = update(&mut st, Msg::DragMove(100.0, 0.0), &ctx());
    let _ = update(&mut st, Msg::DragMove(160.0, 0.0), &ctx());
    assert_eq!(st.layout.left, 264.0 + 60.0);
    let _ = update(&mut st, Msg::DragEnd, &ctx());
    assert!(st.drag.is_none(), "the button came up: the drag ends");
    let _ = update(&mut st, Msg::ResetWidth(Edge::Left), &ctx());
    assert_eq!(st.layout.left, 264.0);
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
                modifiers: Modifiers::CTRL,
                status: Status::Ignored,
            },
            &ctx(),
        );
    };
    // The terminal has the keys: Ctrl+S is the shell's, so nothing is saved.
    st.terminal_hover = true;
    let _ = update(&mut st, Msg::Pressed, &ctx());
    assert!(st.terminal_focus);
    save_key(&mut st);
    assert_eq!(
        st.notice, None,
        "the terminal's Ctrl+S is not the IDE's save"
    );
    // A press elsewhere (the editor) gives the keys back: Ctrl+S is the IDE's save again.
    st.terminal_hover = false;
    let _ = update(&mut st, Msg::Pressed, &ctx());
    assert!(!st.terminal_focus);
    assert!(matches!(
        shortcut(
            &Key::Character("s".into()),
            Modifiers::CTRL,
            Status::Ignored
        ),
        Some(Msg::Save)
    ));
}

#[test]
fn the_drawer_opens_and_folds_and_its_terminal_tabs_are_made_once() {
    let mut st = screen_with_a_tab();
    st.layout.drawer = false;
    assert!(
        st.terminals.is_none(),
        "no shell until the terminal is wanted"
    );
    let _ = update(&mut st, Msg::ToggleDrawer, &ctx());
    assert!(st.layout.drawer);
    assert_eq!(
        st.terminals.as_ref().map(TerminalTabs::len),
        Some(1),
        "opening the drawer makes the first tab"
    );
    let _ = update(&mut st, Msg::ToggleDrawer, &ctx());
    assert!(!st.layout.drawer, "the drawer folds");
    assert_eq!(
        st.terminals.as_ref().map(TerminalTabs::len),
        Some(1),
        "the same tabs go on"
    );
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
fn remaking_the_terminal_tabs_keeps_their_count_and_forgets_their_starts() {
    let mut st = screen_with_a_tab();
    let _ = update(&mut st, Msg::Centre(Centre::Terminal), &ctx());
    let _ = update(&mut st, Msg::Terminals(TabsMsg::Add), &ctx());
    assert_eq!(st.terminals.as_ref().map(TerminalTabs::len), Some(2));
    st.terminal_links.insert(0);
    reopen_terminals(&mut st, &ctx());
    assert_eq!(
        st.terminals.as_ref().map(TerminalTabs::len),
        Some(2),
        "the same two tabs are made again"
    );
    assert!(
        st.terminal_links.is_empty(),
        "the new tabs have not started yet"
    );
}

#[test]
fn the_panel_keys_fold_the_side_panels_only_when_no_field_took_them() {
    let cmd = if cfg!(target_os = "macos") {
        Modifiers::LOGO
    } else {
        Modifiers::CTRL
    };
    let b = Key::Character("b".into());
    let j = Key::Character("j".into());
    assert!(matches!(
        shortcut(&b, cmd, Status::Ignored),
        Some(Msg::ToggleLeft)
    ));
    assert!(matches!(
        shortcut(&j, cmd, Status::Ignored),
        Some(Msg::ToggleRight)
    ));
    assert!(
        shortcut(&b, cmd, Status::Captured).is_none(),
        "a focused field keeps the key"
    );
    assert!(
        shortcut(&b, cmd | Modifiers::SHIFT, Status::Ignored).is_none(),
        "Shift+B is not this key"
    );
}

#[test]
fn open_navigates_to_the_ide_on_the_given_workspace_or_the_picker() {
    assert_eq!(
        go(&Msg::Open(Some("agent:7".into()))),
        Some(Id::Ide(Some("agent:7".into())))
    );
    assert_eq!(go(&Msg::Open(None)), Some(Id::Ide(None)));
    assert_eq!(go(&Msg::Save), None, "other messages do not navigate");
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
fn the_open_shortcut_is_command_shift_e_and_stays_out_of_text_fields() {
    let cmd = if cfg!(target_os = "macos") {
        Modifiers::LOGO
    } else {
        Modifiers::CTRL
    };
    let e = Key::Character("e".into());
    assert!(open_shortcut(&e, cmd | Modifiers::SHIFT, false));
    assert!(
        open_shortcut(&Key::Character("E".into()), cmd | Modifiers::SHIFT, false),
        "Shift makes the key read as E"
    );
    assert!(
        !open_shortcut(&e, cmd | Modifiers::SHIFT, true),
        "a focused text field keeps the key"
    );
    assert!(
        !open_shortcut(&e, cmd, false),
        "without Shift it is not this key"
    );
    assert!(!open_shortcut(
        &e,
        cmd | Modifiers::SHIFT | Modifiers::ALT,
        false
    ));
    assert!(!open_shortcut(
        &Key::Character("f".into()),
        cmd | Modifiers::SHIFT,
        false
    ));
    assert!(
        !open_shortcut(&e, Modifiers::SHIFT, false),
        "Shift and E alone type a letter"
    );
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
