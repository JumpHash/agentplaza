//! Rich text for the terminal (docs/design/workspace.md, section 3.4). The references in agent-written text
//! (GitHub and plaza git links, `@Name`, `#528`, `src/main.rs:42`, library files) become their short labels
//! (`GH o/r #12`, `@Name`, `task #528` shown as `#528`), styled per kind and, where the terminal supports them,
//! wrapped in OSC 8 hyperlinks. Labels, colour and links are for a terminal only: when stdout is piped or
//! redirected, or `--plain` is given, the text prints unchanged, so a script gets the URLs and names as the agent
//! wrote them. JSON output never passes through here.
//!
//! The classifier is `apz-rich`; the same vectors run in the web and the desktop app. Agent names come from the
//! hub, so `@Name` is recognised; without the hub's list the names stay text. Raw text needs no names, so the
//! printers skip the `GET /v1/agents` call unless they will draw labels.

use apz_rich::{AgentName, Ctx as RichCtx, Ref, Span, classify, label};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span as UiSpan;
use serde_json::Value;

use crate::ctx::{Client, Ctx};

/// How agent text is drawn. `plain` prints the raw text (a pipe, a file, or `--plain`); otherwise references become
/// labels, styled when `colour` is on and linked when `hyperlinks` is on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Mode {
    pub plain: bool,
    pub colour: bool,
    pub hyperlinks: bool,
    /// The hub's base URL, without a trailing slash: the web console's hash routes (`#/agents/<cid>`,
    /// `#/tasks/<n>`) are built on it. None: agents and tasks are not linked.
    pub hub: Option<String>,
}

/// What the environment says about the terminal the text goes to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Terminal {
    /// `--plain` was given.
    pub plain: bool,
    pub stdout_is_terminal: bool,
    /// `NO_COLOR` is set (any value).
    pub no_color: bool,
    /// `TERM`, when set.
    pub term: Option<String>,
    /// A Windows console that does not show OSC 8 links (no Windows Terminal, no `TERM_PROGRAM`).
    pub legacy_windows: bool,
    /// `APZ_HYPERLINKS`: `1` forces links on, `0` off, anything else (or unset) decides from the terminal.
    pub hyperlinks_env: Option<String>,
}

impl Terminal {
    /// The terminal as this process sees it.
    pub fn from_env(plain: bool) -> Terminal {
        use std::io::IsTerminal;
        let var = |name: &str| std::env::var(name).ok();
        Terminal {
            plain,
            stdout_is_terminal: std::io::stdout().is_terminal(),
            no_color: var("NO_COLOR").is_some(),
            term: var("TERM"),
            legacy_windows: cfg!(windows)
                && var("WT_SESSION").is_none()
                && var("TERM_PROGRAM").is_none(),
            hyperlinks_env: var("APZ_HYPERLINKS"),
        }
    }
}

impl Mode {
    /// The mode for a terminal and a hub base URL. Labels, colour and links are for a terminal only: a pipe, a
    /// file or `--plain` gets the raw text. On a terminal, colour is off with `NO_COLOR`; links need a terminal
    /// that is not `dumb` and not a legacy Windows console, unless `APZ_HYPERLINKS` says otherwise.
    pub fn decide(terminal: &Terminal, hub: Option<&str>) -> Mode {
        let hub = hub.map(|h| h.trim_end_matches('/').to_string());
        if terminal.plain || !terminal.stdout_is_terminal {
            return Mode {
                plain: true,
                hub,
                ..Mode::default()
            };
        }
        let capable = terminal.term.as_deref() != Some("dumb") && !terminal.legacy_windows;
        let hyperlinks = match terminal.hyperlinks_env.as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => capable,
        };
        Mode {
            plain: false,
            colour: !terminal.no_color,
            hyperlinks,
            hub,
        }
    }
}

/// The classifier's context from the hub: the agents `@Name` may name, and the hub's own host. Empty when the
/// hub cannot answer (the names then stay text).
pub fn context(client: &Client) -> RichCtx {
    let agents = client
        .get("/v1/agents")
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    from_agents(&agents, Some(client.base()))
}

/// The classifier's context from the agents list (`GET /v1/agents`) and the hub's base URL.
pub fn from_agents(agents: &[Value], hub: Option<&str>) -> RichCtx {
    let agents = agents
        .iter()
        .filter_map(|a| {
            let name = a.get("name")?.as_str()?.to_string();
            let cid = a.get("cid")?.as_u64()?;
            Some(AgentName { name, cid })
        })
        .collect();
    RichCtx {
        agents,
        hub_host: hub.map(apz_rich::host_of).filter(|h| !h.is_empty()),
        ..RichCtx::default()
    }
}

/// Whether a printer draws labels, and so needs the hub's agents to resolve `@Name`. `--json` prints the hub's text
/// as it is, and raw text (a pipe, a file, `--plain`) is printed as written: neither needs the call.
pub fn needs_agents(json: bool, mode: &Mode) -> bool {
    !json && !mode.plain
}

/// The context for a printer: the hub's agents when labels are drawn, else nothing (no request is made).
pub fn context_for(ctx: &Ctx) -> RichCtx {
    if needs_agents(ctx.json, &ctx.rich) {
        context(&ctx.client)
    } else {
        RichCtx::default()
    }
}

/// The text with its references replaced by labels, as `mode` draws them. Plain mode returns `text` unchanged.
pub fn render(text: &str, ctx: &RichCtx, mode: &Mode) -> String {
    if mode.plain {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    for span in classify(text, ctx) {
        match span {
            Span::Text(s) | Span::Code(s) => out.push_str(&s),
            Span::Ref { raw, r, .. } => out.push_str(&chip(&raw, &r, ctx, mode)),
        }
    }
    out
}

/// One reference drawn for the terminal: its label, styled, and linked when there is a URL.
fn chip(raw: &str, r: &Ref, ctx: &RichCtx, mode: &Mode) -> String {
    let shown = display(r);
    let styled = if mode.colour {
        format!("\x1b[4;{}m{shown}\x1b[0m", sgr_colour(r))
    } else {
        shown
    };
    match link_of(raw, r, ctx, mode).filter(|_| mode.hyperlinks) {
        Some(url) => format!("\x1b]8;;{url}\x1b\\{styled}\x1b]8;;\x1b\\"),
        None => styled,
    }
}

/// The label a reference shows: `label(ref)`, with `GH ` before the GitHub kinds.
pub fn display(r: &Ref) -> String {
    match r {
        Ref::GithubPull { .. }
        | Ref::GithubIssue { .. }
        | Ref::GithubCommit { .. }
        | Ref::GithubRepo { .. }
        | Ref::GithubFile { .. }
        | Ref::GithubRelease { .. }
        | Ref::GithubRun { .. } => format!("GH {}", label(r)),
        _ => label(r),
    }
}

/// The colour of a kind (SGR parameter): forge references cyan (plaza git bright cyan), agents magenta, tasks
/// yellow, paths green, library files blue, links bright blue.
fn sgr_colour(r: &Ref) -> u8 {
    match r {
        Ref::GithubPull { .. }
        | Ref::GithubIssue { .. }
        | Ref::GithubCommit { .. }
        | Ref::GithubRepo { .. }
        | Ref::GithubFile { .. }
        | Ref::GithubRelease { .. }
        | Ref::GithubRun { .. }
        | Ref::Gist { .. } => 36,
        Ref::PlazaGitPull { .. }
        | Ref::PlazaGitIssue { .. }
        | Ref::PlazaGitCommit { .. }
        | Ref::PlazaGitRepo { .. }
        | Ref::PlazaGitFile { .. }
        | Ref::PlazaGitRelease { .. }
        | Ref::PlazaGitRun { .. } => 96,
        Ref::Agent { .. } | Ref::MentionAll => 35,
        Ref::Task { .. } => 33,
        Ref::Path { .. } => 32,
        Ref::Library { .. } | Ref::LibraryTool { .. } => 34,
        Ref::Image { .. } | Ref::Video { .. } | Ref::Html { .. } | Ref::Link { .. } => 94,
    }
}

/// The ratatui colour of a kind, the same map as `sgr_colour`.
fn ui_colour(r: &Ref) -> Color {
    match sgr_colour(r) {
        36 => Color::Cyan,
        96 => Color::LightCyan,
        35 => Color::Magenta,
        33 => Color::Yellow,
        32 => Color::Green,
        34 => Color::Blue,
        _ => Color::LightBlue,
    }
}

/// The URL a reference links to: the URL it was written as (a bare link or a markdown link's target), else the
/// page it names on the web (GitHub, an agent or a task on the hub). None when no page is known.
fn link_of(raw: &str, r: &Ref, ctx: &RichCtx, mode: &Mode) -> Option<String> {
    if let Some(url) = written_url(raw) {
        return safe(url);
    }
    let hub = mode.hub.as_deref();
    let url = match r {
        Ref::GithubPull {
            owner,
            repo,
            number,
        } => format!("https://github.com/{owner}/{repo}/pull/{number}"),
        Ref::GithubIssue {
            owner,
            repo,
            number,
        } => format!("https://github.com/{owner}/{repo}/issues/{number}"),
        Ref::GithubCommit { owner, repo, sha } => {
            format!("https://github.com/{owner}/{repo}/commit/{sha}")
        }
        Ref::GithubRepo { owner, repo } => format!("https://github.com/{owner}/{repo}"),
        Ref::GithubFile {
            owner,
            repo,
            reference,
            path,
            line,
            dir,
        } => {
            let kind = if *dir { "tree" } else { "blob" };
            let anchor = line.map(|l| format!("#L{l}")).unwrap_or_default();
            format!("https://github.com/{owner}/{repo}/{kind}/{reference}/{path}{anchor}")
        }
        Ref::GithubRelease { owner, repo, tag } => {
            format!("https://github.com/{owner}/{repo}/releases/tag/{tag}")
        }
        Ref::GithubRun { owner, repo, id } => {
            format!("https://github.com/{owner}/{repo}/actions/runs/{id}")
        }
        Ref::Gist { id } => format!("https://gist.github.com/{id}"),
        Ref::Agent { cid, name } => {
            let cid = cid.or_else(|| {
                name.as_deref()
                    .and_then(|n| ctx.agents.iter().find(|a| a.name == n))
                    .map(|a| a.cid)
            })?;
            format!("{}/#/agents/{cid}", hub?)
        }
        Ref::Task { number } => format!("{}/#/tasks/{number}", hub?),
        Ref::Link { url } | Ref::Image { url } | Ref::Video { url } | Ref::Html { url } => {
            url.clone()
        }
        // The plaza git host is not known to the CLI, and paths and library files have no web page of their own.
        _ => return None,
    };
    safe(url)
}

/// The target of a bare `https?://` URL, or of a markdown `[label](url)` or `![alt](url)`.
fn written_url(raw: &str) -> Option<String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    let open = raw.rfind("](")?;
    let target = raw[open + 2..].strip_suffix(')')?;
    (target.starts_with("http://") || target.starts_with("https://")).then(|| target.to_string())
}

/// A URL that can go inside an OSC 8 sequence: no control characters, no spaces.
fn safe(url: String) -> Option<String> {
    url.chars()
        .all(|c| !c.is_control() && !c.is_whitespace())
        .then_some(url)
}

/// The same chips as ratatui spans for the TUI: styled labels, never links.
pub fn spans(text: &str, ctx: &RichCtx) -> Vec<UiSpan<'static>> {
    let mut out = Vec::new();
    for span in classify(text, ctx) {
        match span {
            Span::Text(s) | Span::Code(s) => out.push(UiSpan::raw(s)),
            Span::Ref { r, .. } => {
                let style = Style::default()
                    .fg(ui_colour(&r))
                    .add_modifier(Modifier::UNDERLINED);
                out.push(UiSpan::styled(display(&r), style));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// The shared vectors (`crates/apz-rich/tests/vectors.json`) by name: their text and context.
    fn vector(name: &str) -> (String, RichCtx) {
        let all: Vec<Value> =
            serde_json::from_str(include_str!("../../../crates/apz-rich/tests/vectors.json"))
                .unwrap();
        let v = all
            .iter()
            .find(|v| v["name"] == name)
            .unwrap_or_else(|| panic!("no vector {name}"));
        let text = v["text"].as_str().unwrap().to_string();
        let ctx: RichCtx = serde_json::from_value(v["ctx"].clone()).unwrap();
        (text, ctx)
    }

    fn mode(colour: bool, hyperlinks: bool) -> Mode {
        Mode {
            plain: false,
            colour,
            hyperlinks,
            hub: Some("https://hub.test".into()),
        }
    }

    fn plain() -> Mode {
        Mode {
            plain: true,
            ..Mode::default()
        }
    }

    fn term(stdout_is_terminal: bool) -> Terminal {
        Terminal {
            stdout_is_terminal,
            term: Some("xterm-256color".into()),
            ..Terminal::default()
        }
    }

    #[test]
    fn plain_mode_prints_the_raw_text_unchanged() {
        let (text, ctx) = vector("github_pull");
        assert_eq!(render(&text, &ctx, &plain()), text);
        let (text, ctx) = vector("path_exact");
        assert_eq!(render(&text, &ctx, &plain()), text);
    }

    #[test]
    fn a_github_pull_is_a_short_label_with_gh_and_no_escapes_when_plain_styling_is_off() {
        let (text, ctx) = vector("github_pull");
        assert_eq!(
            render(&text, &ctx, &mode(false, false)),
            "see GH o/r #12 now"
        );
    }

    #[test]
    fn a_styled_github_pull_is_underlined_cyan_and_a_hyperlink_when_links_are_on() {
        let (text, ctx) = vector("github_pull");
        let styled = render(&text, &ctx, &mode(true, false));
        assert_eq!(styled, "see \x1b[4;36mGH o/r #12\x1b[0m now");
        let linked = render(&text, &ctx, &mode(true, true));
        assert_eq!(
            linked,
            "see \x1b]8;;https://github.com/o/r/pull/12\x1b\\\x1b[4;36mGH o/r #12\x1b[0m\x1b]8;;\x1b\\ now"
        );
    }

    #[test]
    fn a_bare_url_links_to_itself_and_a_markdown_link_to_its_target() {
        let (text, ctx) = vector("markdown_link_to_pull");
        let linked = render(&text, &ctx, &mode(false, true));
        assert!(
            linked.contains("\x1b]8;;https://github.com/o/r/pull/12"),
            "{linked:?}"
        );
        assert!(linked.contains("GH o/r #12"), "{linked:?}");
    }

    #[test]
    fn each_kind_has_its_label_in_rich_mode() {
        for (name, expected) in [
            ("github_issue", "GH o/r #7"),
            ("github_commit", "GH o/r @abc1234"),
            ("github_repo", "GH o/r"),
            ("github_run", "GH o/r run 9"),
            ("gist", "gist abc123"),
            ("link_bare", "example.com/docs/intro"),
            ("mention_all", "@all"),
            ("path_exact", "src/main.rs"),
            ("path_line_colon_and_trailing_period", "src/main.rs:42"),
        ] {
            let (text, ctx) = vector(name);
            let shown = render(&text, &ctx, &mode(false, false));
            assert!(
                shown.contains(expected),
                "{name}: {shown:?} lacks {expected:?}"
            );
        }
    }

    #[test]
    fn a_task_and_an_agent_link_to_the_hub_console() {
        let (text, ctx) = vector("hash_number_in_project_is_task");
        let linked = render(&text, &ctx, &mode(false, true));
        assert!(
            linked.contains("\x1b]8;;https://hub.test/#/tasks/"),
            "{linked:?}"
        );
        let (text, ctx) = vector("agent_known");
        let linked = render(&text, &ctx, &mode(false, true));
        assert!(linked.contains("@Ada"), "{linked:?}");
        assert!(
            linked.contains("\x1b]8;;https://hub.test/#/agents/"),
            "{linked:?}"
        );
    }

    #[test]
    fn a_path_is_styled_green_and_not_linked() {
        let (text, ctx) = vector("path_exact");
        let linked = render(&text, &ctx, &mode(true, true));
        assert_eq!(linked, "edit \x1b[4;32msrc/main.rs\x1b[0m");
    }

    #[test]
    fn code_spans_and_plain_text_are_never_rewritten() {
        // The code span keeps its `#7` as written; the agent outside it is still a chip.
        let (text, ctx) = vector("code_span_hides_refs");
        let shown = render(&text, &ctx, &mode(true, true));
        assert!(shown.starts_with("`#7` and "), "{shown:?}");
        assert!(shown.contains("@Ada"), "{shown:?}");
        let (text, ctx) = vector("code_span_untouched");
        assert_eq!(render(&text, &ctx, &mode(true, true)), text);
        let (text, ctx) = vector("plain_text");
        assert_eq!(render(&text, &ctx, &mode(true, true)), text);
    }

    #[test]
    fn a_terminal_gets_labels_colour_and_links() {
        let tty = Mode::decide(&term(true), Some("https://hub.test/"));
        assert_eq!(
            tty,
            Mode {
                plain: false,
                colour: true,
                hyperlinks: true,
                hub: Some("https://hub.test".into())
            }
        );
        let no_colour = Mode::decide(
            &Terminal {
                no_color: true,
                ..term(true)
            },
            None,
        );
        assert!(!no_colour.colour && no_colour.hyperlinks && !no_colour.plain);
        let dumb = Mode::decide(
            &Terminal {
                term: Some("dumb".into()),
                ..term(true)
            },
            None,
        );
        assert!(!dumb.hyperlinks && !dumb.plain);
        let legacy = Mode::decide(
            &Terminal {
                legacy_windows: true,
                ..term(true)
            },
            None,
        );
        assert!(!legacy.hyperlinks && !legacy.plain);
        let forced_off = Mode::decide(
            &Terminal {
                hyperlinks_env: Some("0".into()),
                ..term(true)
            },
            None,
        );
        assert!(!forced_off.hyperlinks && forced_off.colour && !forced_off.plain);
        let forced_on = Mode::decide(
            &Terminal {
                hyperlinks_env: Some("1".into()),
                ..term(true)
            },
            None,
        );
        assert!(forced_on.hyperlinks && !forced_on.plain);
    }

    #[test]
    fn a_pipe_or_a_file_gets_the_raw_text_whatever_the_environment_says() {
        // Stdout is not a terminal: no labels, colour or escapes, even when links are forced on.
        for env in [None, Some("1"), Some("0")] {
            let pipe = Mode::decide(
                &Terminal {
                    hyperlinks_env: env.map(String::from),
                    ..term(false)
                },
                Some("https://hub.test"),
            );
            assert!(pipe.plain, "{env:?}: a pipe is plain");
            assert!(!pipe.colour && !pipe.hyperlinks, "{env:?}: no escapes");
            assert_eq!(pipe.hub.as_deref(), Some("https://hub.test"));
        }
        // `--plain` on a terminal is the same as a pipe.
        let flag = Mode::decide(
            &Terminal {
                plain: true,
                hyperlinks_env: Some("1".into()),
                ..term(true)
            },
            None,
        );
        assert!(flag.plain && !flag.colour && !flag.hyperlinks);
    }

    #[test]
    fn a_pipe_prints_every_kind_of_reference_as_written() {
        let pipe = Mode::decide(&term(false), None);
        for name in [
            "github_pull",
            "github_issue",
            "github_commit",
            "github_repo",
            "github_run",
            "gist",
            "link_bare",
            "markdown_link_to_pull",
            "mention_all",
            "path_exact",
            "path_line_colon_and_trailing_period",
            "hash_number_in_project_is_task",
            "agent_known",
            "code_span_hides_refs",
        ] {
            let (text, ctx) = vector(name);
            assert_eq!(render(&text, &ctx, &pipe), text, "{name}");
        }
    }

    #[test]
    fn only_a_terminal_that_draws_labels_asks_the_hub_for_agents() {
        let tty = Mode::decide(&term(true), None);
        let pipe = Mode::decide(&term(false), None);
        let flag = Mode::decide(
            &Terminal {
                plain: true,
                ..term(true)
            },
            None,
        );
        assert!(needs_agents(false, &tty), "a terminal resolves @Name");
        assert!(!needs_agents(false, &pipe), "a pipe needs no names");
        assert!(!needs_agents(false, &flag), "--plain needs no names");
        assert!(!needs_agents(true, &tty), "--json needs no names");
    }

    #[test]
    fn spans_for_the_tui_are_styled_labels_and_never_links() {
        let (text, ctx) = vector("github_pull");
        let spans = spans(&text, &ctx);
        let shown: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(shown, "see GH o/r #12 now");
        assert!(spans.iter().all(|s| !s.content.contains('\x1b')));
    }
}
