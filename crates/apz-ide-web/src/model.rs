//! The JSON layer over apz-ide. Each function takes and returns JSON text (or plain text where the value is a string),
//! and reports a refusal as its message. Nothing here is wasm-specific, so the tests run natively.

use apz_ide::{Action, ChangeSet, Entry, IdeState, WorkspaceId};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Reads a JSON argument; the message names which argument did not parse.
fn read<T: DeserializeOwned>(what: &str, json: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("bad {what} JSON: {e}"))
}

/// Writes a JSON result.
fn write<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

/// Parses `git diff` or `diff -u` text. Returns the ChangeSet JSON with a `stats` field (files, additions, deletions).
pub fn parse_unified(text: &str) -> Result<String, String> {
    let set = apz_ide::parse_unified(text).map_err(|e| e.to_string())?;
    let mut out = serde_json::to_value(&set).map_err(|e| e.to_string())?;
    out["stats"] = serde_json::to_value(set.stats()).map_err(|e| e.to_string())?;
    write(&out)
}

pub fn check_path(p: &str) -> Result<String, String> {
    apz_ide::check_path(p).map_err(|e| e.to_string())
}

pub fn check_read(p: &str) -> Result<String, String> {
    apz_ide::check_read(p).map_err(|e| e.to_string())
}

pub fn check_write(p: &str) -> Result<String, String> {
    apz_ide::check_write(p).map_err(|e| e.to_string())
}

pub fn check_dir(p: &str) -> Result<String, String> {
    apz_ide::check_dir(p).map_err(|e| e.to_string())
}

pub fn is_secret(p: &str) -> bool {
    apz_ide::is_secret(p)
}

pub fn initial_state() -> String {
    // An IdeState of plain strings and enums: serializing it cannot fail.
    serde_json::to_string(&IdeState::default()).unwrap_or_default()
}

/// Applies an Action (JSON) to an IdeState (JSON); returns the next IdeState as JSON.
pub fn reduce(state_json: &str, action_json: &str) -> Result<String, String> {
    let state: IdeState = read("state", state_json)?;
    let action: Action = read("action", action_json)?;
    write(&apz_ide::reduce(state, action))
}

/// The direct children of `dir` among an Entry array (JSON), in display order.
pub fn children(entries_json: &str, dir: &str) -> Result<String, String> {
    let entries: Vec<Entry> = read("entries", entries_json)?;
    write(&apz_ide::children(&entries, dir))
}

/// The entries a ChangeSet (JSON, `stats` ignored) touches, with their ancestor directories.
pub fn changed_filter(entries_json: &str, changes_json: &str) -> Result<String, String> {
    let entries: Vec<Entry> = read("entries", entries_json)?;
    let changes: ChangeSet = read("changes", changes_json)?;
    write(&apz_ide::changed_filter(&entries, &changes))
}

/// A workspace id's JSON form, which is its canonical string quoted (`"agent:7"`).
pub fn parse_workspace_id(s: &str) -> Result<String, String> {
    let id: WorkspaceId = s.parse().map_err(|e: apz_ide::IdError| e.to_string())?;
    write(&id)
}

/// The canonical text of a workspace id given as its JSON form.
pub fn format_workspace_id(json: &str) -> Result<String, String> {
    let id: WorkspaceId = read("workspace id", json)?;
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_ide::{Base, Panel};

    /// A `git diff` with a modified file (one line removed, one added) and a new file.
    const DIFF: &str = "diff --git a/src/a.txt b/src/a.txt
index 1111111..2222222 100644
--- a/src/a.txt
+++ b/src/a.txt
@@ -1,2 +1,2 @@ fn main
 keep
-old
+new
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
";

    #[test]
    fn parse_unified_round_trips_the_change_set_and_adds_stats() {
        let out = parse_unified(DIFF).unwrap();
        // The ChangeSet shape is apz-ide's: the extra `stats` field is ignored when it is read back.
        let back: ChangeSet = serde_json::from_str(&out).unwrap();
        assert_eq!(back, apz_ide::parse_unified(DIFF).unwrap());
        assert_eq!(back.files.len(), 2);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["stats"],
            serde_json::json!({"files": 2, "additions": 2, "deletions": 1})
        );
        assert_eq!(v["files"][1]["status"], "added");
        assert_eq!(v["files"][0]["hunks"][0]["lines"][0]["kind"], "context");
    }

    #[test]
    fn path_checks_clean_or_refuse() {
        assert_eq!(check_path("a/./b//c/").unwrap(), "a/b/c");
        assert_eq!(check_path("../x").unwrap_err(), "path may not contain ..");
        assert_eq!(
            check_path("/etc/passwd").unwrap_err(),
            "path must be relative"
        );
        assert_eq!(check_dir("").unwrap(), "");
        assert_eq!(check_dir("src/").unwrap(), "src");
        assert!(check_dir(".ssh").is_err());
        assert!(check_read(".env").is_err());
        assert!(check_read("src/key.pem").is_err());
        assert_eq!(check_read("src/main.rs").unwrap(), "src/main.rs");
        assert!(check_write(".plaza/notes.md").is_err());
        assert!(check_write("src/.plaza/x").is_err());
        assert_eq!(check_write("src/main.rs").unwrap(), "src/main.rs");
        assert!(is_secret("deploy/id_ed25519"));
        assert!(!is_secret("deploy/readme.md"));
    }

    #[test]
    fn reduce_applies_actions_to_the_json_state() {
        let s = initial_state();
        let s = reduce(&s, r#"{"open":"a.txt"}"#).unwrap();
        let s = reduce(&s, r#"{"open":"b.txt"}"#).unwrap();
        let s = reduce(&s, r#"{"activate":"a.txt"}"#).unwrap();
        let st: IdeState = serde_json::from_str(&s).unwrap();
        assert_eq!(st.open, ["a.txt", "b.txt"]);
        assert_eq!(st.active.as_deref(), Some("a.txt"));
        assert_eq!(st.panel, Panel::Changes);
        assert_eq!(st.base, Base::WorkStart);

        let s = reduce(&s, r#"{"set_panel":"terminal"}"#).unwrap();
        let s = reduce(&s, r#"{"set_base":{"checkpoint":3}}"#).unwrap();
        let s = reduce(&s, r#""toggle_review""#).unwrap();
        let st: IdeState = serde_json::from_str(&s).unwrap();
        assert_eq!(st.panel, Panel::Terminal);
        assert_eq!(st.base, Base::Checkpoint(3));
        assert!(st.review);

        let s = reduce(&s, r#"{"set_workspace":"repo:web@main"}"#).unwrap();
        let st: IdeState = serde_json::from_str(&s).unwrap();
        assert_eq!(
            st.workspace.map(|w| w.to_string()).as_deref(),
            Some("repo:web@main")
        );
        assert!(st.open.is_empty() && st.active.is_none());
    }

    #[test]
    fn workspace_ids_round_trip_in_their_canonical_form() {
        assert_eq!(parse_workspace_id("agent:7").unwrap(), "\"agent:7\"");
        assert_eq!(format_workspace_id("\"agent:7\"").unwrap(), "agent:7");
        let json = parse_workspace_id("repo:web@feature/x").unwrap();
        assert_eq!(format_workspace_id(&json).unwrap(), "repo:web@feature/x");
        assert!(parse_workspace_id("agent:07").is_err());
        assert!(parse_workspace_id("nope").is_err());
        assert!(format_workspace_id("7").is_err());
    }
}
