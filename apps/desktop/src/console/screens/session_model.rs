//! The sessions screens' pure logic, as the web's `apps/web/src/views/session-model.ts` has it: a session's state (its
//! label and tone, whether it is over), the list's rows (the scope, the live filter, the order), its seat and cost text,
//! a route's reference (a VM's CID or a session id), the transcript's lines (a label, a text and a note for each kind)
//! and a context's fill. No drawing here, so the screens and the tests below say the same things as the web.

use serde_json::Value;

use crate::console::chat_tools::tokens_text;
use crate::console::roles;
use crate::console::ui::{self, Tone};
use crate::console::work_api::{
    Context, ContextPart, ContextTool, PausedBy, Session, SessionKind, SessionState, Usage,
};

/// The project the main orchestrator's session works for: the whole plaza, not a project of its own.
pub const PLAZA_PROJECT: &str = "plaza";

/// The longest tool output a log row shows; the rest is clipped.
const OUTPUT_CHARS: usize = 400;

/// A session's state in words: a session that is starting reads "Starting".
pub fn state_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Queued => "Queued",
        SessionState::Placing | SessionState::Forking => "Starting",
        SessionState::Running => "Running",
        SessionState::Paused => "Paused",
        SessionState::Compacting => "Compacting",
        SessionState::HandingOver => "Handing over",
        SessionState::Ending => "Ending",
        SessionState::Ended => "Ended",
        SessionState::Failed => "Failed",
    }
}

/// The tone of a session's state: a paused one warns, a failed one is danger, a queued or ended one is quiet.
pub fn state_tone(state: SessionState) -> Tone {
    match state {
        SessionState::Paused => Tone::Warn,
        SessionState::Failed => Tone::Danger,
        SessionState::Queued | SessionState::Ended => Tone::Muted,
        _ => Tone::Info,
    }
}

/// Whether a session is over: ended or failed, and nothing more happens to it.
pub fn is_final(state: SessionState) -> bool {
    matches!(state, SessionState::Ended | SessionState::Failed)
}

/// Whether a session is live: not over yet.
pub fn is_live(session: &Session) -> bool {
    !is_final(session.state)
}

/// Whether a session works for `project` (None: every project).
pub fn in_scope(session: &Session, project: Option<&str>) -> bool {
    project.is_none_or(|p| session.project == p)
}

/// The sessions a list shows: those in scope, the live ones only when asked, live first and the newest first within
/// each group. The sessions are not reordered; the rows are references into them.
pub fn rows<'a>(
    sessions: &'a [Session],
    project: Option<&str>,
    live_only: bool,
) -> Vec<&'a Session> {
    let mut rows: Vec<&Session> = sessions
        .iter()
        .filter(|s| in_scope(s, project) && (!live_only || is_live(s)))
        .collect();
    rows.sort_by_key(|s| (is_final(s.state), std::cmp::Reverse(s.created_ms)));
    rows
}

/// The seat a session fills, as a list shows it: `long-lived` for a long session, `helper` for a helper, else the seat.
pub fn seat_text(session: &Session) -> String {
    match session.kind {
        SessionKind::Long => "long-lived".into(),
        SessionKind::Helper => "helper".into(),
        SessionKind::Step => session.seat.clone().unwrap_or_else(|| "—".into()),
    }
}

/// A session's heading: its role and its seat, as `Builder · builder` or `Orchestrator · long-lived`.
pub fn heading(session: &Session) -> String {
    format!("{} · {}", roles::label(&session.role), seat_text(session))
}

/// Why a session waits, in a few words; none when no reason is set.
pub fn paused_text(by: Option<PausedBy>) -> Option<&'static str> {
    by.map(|by| match by {
        PausedBy::Budget => "Budget reached",
        PausedBy::Allowance => "Allowance reached",
        PausedBy::Disk => "Disk full",
        PausedBy::Card => "Waiting for you",
        PausedBy::Node => "Node unavailable",
    })
}

/// Micro-dollars as dollars with two decimals, to the cent: 1500000 is "$1.50".
pub fn cost_text(micros: u64) -> String {
    let cents = micros.saturating_add(5_000) / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

/// A session's tokens in and out, as the list shows them: "41.2k in · 5.3k out".
pub fn usage_text(usage: &Usage) -> String {
    format!(
        "{} in · {} out",
        tokens_text(usage.tokens_in),
        tokens_text(usage.tokens_out)
    )
}

/// A count with its noun: "1 turn", "3 turns".
pub fn count_text(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Whether a route's reference is a VM's CID (digits, as an agent's old link `#/sessions/105` gave) rather than a
/// session id.
pub fn is_cid_ref(reference: &str) -> bool {
    !reference.is_empty() && reference.bytes().all(|b| b.is_ascii_digit())
}

/// The newest session on VM `cid`, or none when none runs there.
pub fn newest_with_cid(sessions: &[Session], cid: u64) -> Option<&Session> {
    sessions
        .iter()
        .filter(|s| s.cid == Some(cid))
        .fold(None, |newest: Option<&Session>, s| match newest {
            Some(n) if n.created_ms >= s.created_ms => Some(n),
            _ => Some(s),
        })
}

/// The share of the window the last call filled, from 0 to 1; none when the window is unknown.
pub fn context_fill(ctx: &Context) -> Option<f64> {
    let share = match ctx.fill {
        Some(fill) if fill.is_finite() => Some(fill),
        _ => ctx
            .window
            .filter(|w| *w > 0)
            .map(|w| ctx.used as f64 / w as f64),
    };
    share.map(|s| s.clamp(0.0, 1.0))
}

/// Whether a session has made a model call yet: the call count says so, a CID alone does not.
pub fn has_call(ctx: &Context) -> bool {
    ctx.calls > 0
}

/// The tools of a context, the ones that sent the most result tokens first.
pub fn tools_by_result(tools: &[ContextTool]) -> Vec<ContextTool> {
    let mut sorted = tools.to_vec();
    sorted.sort_by(|a, b| {
        b.result_tokens
            .cmp(&a.result_tokens)
            .then_with(|| a.tool.cmp(&b.tool))
    });
    sorted
}

/// The context's parts in the breakdown's order (system first, summary last). A part the hub did not send counts 0.
pub fn ordered_parts(parts: &[ContextPart]) -> Vec<(Part, u64)> {
    Part::ALL
        .iter()
        .map(|part| {
            let tokens = parts
                .iter()
                .find(|p| p.part == part.key())
                .map_or(0, |p| p.tokens);
            (*part, tokens)
        })
        .collect()
}

/// One part of a session's context, as the breakdown names it. A part the console does not know is not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    System,
    Tools,
    History,
    ToolResults,
    Attachments,
    Summary,
}

impl Part {
    /// The breakdown's order: system first, summary last.
    pub const ALL: [Part; 6] = [
        Part::System,
        Part::Tools,
        Part::History,
        Part::ToolResults,
        Part::Attachments,
        Part::Summary,
    ];

    /// The part's name in the hub's data.
    pub fn key(self) -> &'static str {
        match self {
            Part::System => "system",
            Part::Tools => "tools",
            Part::History => "history",
            Part::ToolResults => "tool_results",
            Part::Attachments => "attachments",
            Part::Summary => "summary",
        }
    }

    /// The part's name as the breakdown shows it.
    pub fn name(self) -> &'static str {
        match self {
            Part::System => "System",
            Part::Tools => "Tools",
            Part::History => "History",
            Part::ToolResults => "Tool results",
            Part::Attachments => "Attachments",
            Part::Summary => "Summary",
        }
    }
}

/// A clock reading of a timestamp in UTC, `14:03:07`: the timeline's clock, as the Tasks screen draws its own.
pub fn clock_text(ms: u64) -> String {
    let s = (ms / 1000) % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

// ---- The transcript ---------------------------------------------------------

/// One row of a session's log: what it is (`label`), what it says (`text`, in mono when it is code or a tool's output),
/// a fact beside the label (`note`), whether it is an alert, and whether this console knows its kind (`known`: a kind
/// it does not draw keeps its own name and no text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub label: String,
    pub text: String,
    pub mono: bool,
    pub note: String,
    pub alert: bool,
    pub known: bool,
}

impl Line {
    fn labelled(label: &str) -> Line {
        Line {
            label: label.into(),
            text: String::new(),
            mono: false,
            note: String::new(),
            alert: false,
            known: true,
        }
    }
}

/// A transcript entry as the log draws it. The kinds this console draws get a label and a text (the prompt, the model's
/// words, a tool's call and its output, a decision, an error, the end of the work, a compression); any other kind keeps
/// its own name and no text, and the expert view shows its data.
pub fn transcript_line(kind: &str, data: &Value) -> Line {
    let text = |key: &str| data[key].as_str().unwrap_or("").to_string();
    match kind {
        "work_start" => Line {
            text: text("prompt"),
            ..Line::labelled("Started")
        },
        "model_response" => Line {
            text: model_text(&data["content"]),
            note: token_note(&data["usage"]),
            ..Line::labelled("Model")
        },
        "tool_request" => {
            let call = [text("name"), json_text(&data["input"])]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            Line {
                text: call,
                mono: true,
                ..Line::labelled("Tool call")
            }
        }
        "tool_result" => {
            let ok = data["ok"] == Value::Bool(true);
            Line {
                text: result_text(&data["output"]),
                mono: true,
                alert: !ok,
                ..Line::labelled(if ok { "Tool result" } else { "Tool failed" })
            }
        }
        "tool_decision" => Line {
            text: decision_text(&data["decision"], &data["output"]),
            ..Line::labelled("Decision")
        },
        "error" => Line {
            text: text("message"),
            alert: true,
            ..Line::labelled("Error")
        },
        "work_end" => {
            let status = text("status");
            Line {
                text: text("summary"),
                note: outcome_word(&status),
                alert: status == "failed",
                ..Line::labelled("Work ended")
            }
        }
        "compressed" => Line {
            text: format!(
                "{} → {} tokens, {} folded",
                tokens_text(num(&data["before"])),
                tokens_text(num(&data["after"])),
                count_text(num(&data["folded"]), "turn", "turns"),
            ),
            ..Line::labelled("Compressed")
        },
        other => Line {
            label: other.into(),
            known: false,
            ..Line::labelled("")
        },
    }
}

/// A model response's words: its text parts joined by line breaks (a plain string is itself). Tool calls are no words.
pub fn model_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A tool's output as the log shows it: its text, or its JSON, clipped to 400 characters.
pub fn result_text(output: &Value) -> String {
    let raw = match output {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    ui::ellipsize(&raw, OUTPUT_CHARS)
}

/// A model call's tokens as a note: "3 in · 1 out". Empty when the entry carries no usage.
fn token_note(usage: &Value) -> String {
    if !usage.is_object() {
        return String::new();
    }
    format!(
        "{} in · {} out",
        tokens_text(num(&usage["input_tokens"])),
        tokens_text(num(&usage["output_tokens"])),
    )
}

/// A decision with what it produced: "Allowed", or "Denied: the output".
fn decision_text(decision: &Value, output: &Value) -> String {
    let word = match decision.as_str().unwrap_or("") {
        "allow" => "Allowed".to_string(),
        "deny" => "Denied".to_string(),
        "done" => "Done".to_string(),
        "pending" => "Pending".to_string(),
        other => other.to_string(),
    };
    let out = result_text(output);
    if out.is_empty() {
        word
    } else {
        format!("{word}: {out}")
    }
}

/// A work end's status in words; a status the console does not know shows as it came.
fn outcome_word(status: &str) -> String {
    match status {
        "done" => "Done".into(),
        "failed" => "Failed".into(),
        "stopped" => "Stopped".into(),
        other => other.into(),
    }
}

/// A value as compact JSON, or empty when there is none.
fn json_text(value: &Value) -> String {
    if value.is_null() {
        String::new()
    } else {
        value.to_string()
    }
}

/// A count in the data of an entry; a value that is not a whole number counts 0.
fn num(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn session(id: &str, state: SessionState, created_ms: u64) -> Session {
        Session {
            id: id.into(),
            project: "pr1".into(),
            role: "builder".into(),
            seat: Some("builder".into()),
            state,
            created_ms,
            ..Default::default()
        }
    }

    #[test]
    fn a_session_is_labelled_and_toned_by_its_state() {
        assert_eq!(state_label(SessionState::Placing), "Starting");
        assert_eq!(state_label(SessionState::Forking), "Starting");
        assert_eq!(state_label(SessionState::HandingOver), "Handing over");
        assert_eq!(state_tone(SessionState::Paused), Tone::Warn);
        assert_eq!(state_tone(SessionState::Failed), Tone::Danger);
        assert_eq!(state_tone(SessionState::Running), Tone::Info);
        assert_eq!(state_tone(SessionState::Ended), Tone::Muted);
    }

    #[test]
    fn only_ended_and_failed_sessions_are_over() {
        assert!(is_final(SessionState::Ended));
        assert!(is_final(SessionState::Failed));
        assert!(!is_final(SessionState::Paused));
        assert!(is_live(&session("s1", SessionState::Queued, 0)));
        assert!(!is_live(&session("s1", SessionState::Ended, 0)));
    }

    #[test]
    fn the_list_is_the_scope_with_live_sessions_first_and_the_newest_first() {
        let mut other = session("s9", SessionState::Running, 900);
        other.project = "pr2".into();
        let all = vec![
            session("s1", SessionState::Ended, 100),
            session("s2", SessionState::Running, 200),
            session("s3", SessionState::Failed, 300),
            session("s4", SessionState::Paused, 150),
            other,
        ];
        let ids = |rows: Vec<&Session>| rows.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(rows(&all, None, false)), ["s9", "s2", "s4", "s3", "s1"]);
        assert_eq!(
            ids(rows(&all, Some("pr1"), false)),
            ["s2", "s4", "s3", "s1"]
        );
        assert_eq!(ids(rows(&all, Some("pr1"), true)), ["s2", "s4"]);
        assert_eq!(all.len(), 5, "the sessions are not reordered");
    }

    #[test]
    fn a_seat_reads_as_its_seat_or_as_long_lived_or_helper() {
        assert_eq!(
            seat_text(&session("s1", SessionState::Running, 0)),
            "builder"
        );
        let mut planner = session("s2", SessionState::Running, 0);
        planner.kind = SessionKind::Long;
        planner.seat = None;
        assert_eq!(seat_text(&planner), "long-lived");
        let mut helper = session("s3", SessionState::Running, 0);
        helper.kind = SessionKind::Helper;
        assert_eq!(seat_text(&helper), "helper");
        let mut unseated = session("s4", SessionState::Queued, 0);
        unseated.seat = None;
        assert_eq!(seat_text(&unseated), "—");
    }

    #[test]
    fn a_heading_names_the_role_and_the_seat() {
        let mut reviewer = session("s1", SessionState::Running, 0);
        reviewer.role = "reviewer".into();
        reviewer.seat = Some("reviewer:1".into());
        assert_eq!(heading(&reviewer), "Reviewer · reviewer:1");
        let mut orchestrator = session("s2", SessionState::Running, 0);
        orchestrator.role = "orchestrator".into();
        orchestrator.kind = SessionKind::Long;
        assert_eq!(heading(&orchestrator), "Orchestrator · long-lived");
    }

    #[test]
    fn a_paused_session_says_why_it_waits() {
        assert_eq!(paused_text(Some(PausedBy::Budget)), Some("Budget reached"));
        assert_eq!(paused_text(Some(PausedBy::Card)), Some("Waiting for you"));
        assert_eq!(paused_text(None), None);
    }

    #[test]
    fn micro_dollars_read_as_dollars_to_the_cent() {
        assert_eq!(cost_text(1_500_000), "$1.50");
        assert_eq!(cost_text(0), "$0.00");
        assert_eq!(cost_text(4_999), "$0.00");
        assert_eq!(cost_text(5_000), "$0.01");
        assert_eq!(cost_text(12_345_678), "$12.35");
    }

    #[test]
    fn usage_reads_tokens_in_and_out() {
        let usage = Usage {
            tokens_in: 41_200,
            tokens_out: 5_300,
            ..Default::default()
        };
        assert_eq!(usage_text(&usage), "41.2k in · 5.3k out");
        assert_eq!(count_text(1, "turn", "turns"), "1 turn");
        assert_eq!(count_text(3, "turn", "turns"), "3 turns");
    }

    #[test]
    fn a_route_reference_is_a_cid_when_it_is_all_digits() {
        assert!(is_cid_ref("105"));
        assert!(!is_cid_ref("s12"));
        assert!(!is_cid_ref(""));
    }

    #[test]
    fn the_newest_session_on_a_vm_is_the_one_created_last() {
        let mut a = session("s1", SessionState::Ended, 100);
        a.cid = Some(105);
        let mut b = session("s2", SessionState::Running, 300);
        b.cid = Some(105);
        let mut c = session("s3", SessionState::Running, 400);
        c.cid = Some(106);
        let all = [a, b, c];
        assert_eq!(
            newest_with_cid(&all, 105).map(|s| s.id.as_str()),
            Some("s2")
        );
        assert_eq!(newest_with_cid(&all, 107).map(|s| s.id.as_str()), None);
    }

    #[test]
    fn the_fill_is_the_hub_s_or_used_over_the_window_at_most_one() {
        let known = Context {
            used: 50_000,
            window: Some(200_000),
            ..Default::default()
        };
        assert_eq!(context_fill(&known), Some(0.25));
        let told = Context {
            fill: Some(1.4),
            ..Default::default()
        };
        assert_eq!(context_fill(&told), Some(1.0));
        let unknown = Context {
            used: 9,
            ..Default::default()
        };
        assert_eq!(context_fill(&unknown), None);
    }

    #[test]
    fn a_context_has_a_call_once_it_has_made_one() {
        assert!(!has_call(&Context {
            cid: Some(105),
            ..Default::default()
        }));
        assert!(has_call(&Context {
            calls: 2,
            ..Default::default()
        }));
    }

    #[test]
    fn the_tools_that_sent_the_most_result_tokens_come_first() {
        let tools = vec![
            ContextTool {
                tool: "read".into(),
                calls: 1,
                errors: 0,
                result_tokens: 10,
            },
            ContextTool {
                tool: "run".into(),
                calls: 4,
                errors: 1,
                result_tokens: 900,
            },
            ContextTool {
                tool: "edit".into(),
                calls: 2,
                errors: 0,
                result_tokens: 10,
            },
        ];
        let names: Vec<String> = tools_by_result(&tools)
            .into_iter()
            .map(|t| t.tool)
            .collect();
        assert_eq!(names, ["run", "edit", "read"]);
    }

    #[test]
    fn the_parts_come_in_the_breakdown_s_order_and_the_missing_ones_count_nothing() {
        let parts = vec![
            ContextPart {
                part: "summary".into(),
                tokens: 7,
            },
            ContextPart {
                part: "history".into(),
                tokens: 500,
            },
            ContextPart {
                part: "made_up".into(),
                tokens: 99,
            },
        ];
        let got = ordered_parts(&parts);
        assert_eq!(got.len(), 6, "an unknown part is dropped");
        assert_eq!(got[0], (Part::System, 0));
        assert_eq!(got[2], (Part::History, 500));
        assert_eq!(got[5], (Part::Summary, 7));
        assert_eq!(Part::ToolResults.name(), "Tool results");
        assert_eq!(Part::ToolResults.key(), "tool_results");
    }

    #[test]
    fn the_clock_reads_the_time_of_day() {
        assert_eq!(clock_text(0), "00:00:00");
        assert_eq!(clock_text(3_723_000), "01:02:03");
        assert_eq!(clock_text(86_400_000 + 3_723_000), "01:02:03");
    }

    #[test]
    fn a_model_turn_reads_its_text_parts_and_its_tokens() {
        let content = json!([
            { "type": "text", "text": "Looking." },
            { "type": "tool_use", "name": "read" },
            { "type": "text", "text": "Done." },
        ]);
        assert_eq!(model_text(&content), "Looking.\nDone.");
        assert_eq!(model_text(&json!("plain")), "plain");
        let line = transcript_line(
            "model_response",
            &json!({ "content": "hi", "usage": { "input_tokens": 3, "output_tokens": 1 } }),
        );
        assert_eq!(line.label, "Model");
        assert_eq!(line.text, "hi");
        assert_eq!(line.note, "3 in · 1 out");
    }

    #[test]
    fn a_tool_call_and_its_result_are_mono_and_a_failed_one_alerts() {
        let call = transcript_line(
            "tool_request",
            &json!({ "name": "run", "input": { "cmd": "ls" } }),
        );
        assert_eq!(call.label, "Tool call");
        assert_eq!(call.text, "run {\"cmd\":\"ls\"}");
        assert!(call.mono && !call.alert);
        let ok = transcript_line("tool_result", &json!({ "ok": true, "output": "a\nb" }));
        assert_eq!((ok.label.as_str(), ok.alert), ("Tool result", false));
        let failed = transcript_line("tool_result", &json!({ "ok": false, "output": "boom" }));
        assert_eq!((failed.label.as_str(), failed.alert), ("Tool failed", true));
    }

    #[test]
    fn a_long_tool_output_is_clipped_and_a_decision_says_what_it_decided() {
        let long = "x".repeat(500);
        let clipped = result_text(&json!(long));
        assert_eq!(clipped.chars().count(), 400);
        assert!(clipped.ends_with('…'));
        let allowed = transcript_line("tool_decision", &json!({ "decision": "allow" }));
        assert_eq!(allowed.text, "Allowed");
        let denied = transcript_line(
            "tool_decision",
            &json!({ "decision": "deny", "output": "not here" }),
        );
        assert_eq!(denied.text, "Denied: not here");
    }

    #[test]
    fn a_work_end_names_its_outcome_and_a_failure_alerts() {
        let done = transcript_line(
            "work_end",
            &json!({ "status": "done", "summary": "Shipped" }),
        );
        assert_eq!(
            (done.label.as_str(), done.note.as_str(), done.alert),
            ("Work ended", "Done", false)
        );
        let failed = transcript_line("work_end", &json!({ "status": "failed" }));
        assert_eq!(failed.note, "Failed");
        assert!(failed.alert);
    }

    #[test]
    fn a_compression_says_how_many_tokens_it_saved() {
        let line = transcript_line(
            "compressed",
            &json!({ "before": 41_200, "after": 9_000, "folded": 12 }),
        );
        assert_eq!(line.text, "41.2k → 9k tokens, 12 turns folded");
    }

    #[test]
    fn a_kind_the_console_does_not_draw_keeps_its_name_and_no_text() {
        let line = transcript_line("novel_kind", &json!({ "x": 1 }));
        assert_eq!(line.label, "novel_kind");
        assert!(!line.known);
        assert!(line.text.is_empty());
    }
}
