//! The rules for listing applications: their names, their scripting bridges, and the order and uniqueness of the list.
//!
//! The macOS code reads each bundle's `Info.plist` and folder into plain values and hands them here.

use apz_desktop::AppInfo;
use apz_station::Bridge;

/// The scripting bridges an app offers. JXA (JavaScript for Automation) works with any app that can be scripted at
/// all, so it is always offered. AppleScript needs the app to enable it (`NSAppleScriptEnabled`) or to publish a
/// scripting dictionary (an `.sdef` file).
pub fn scripting_bridges(applescript_enabled: bool, has_scripting_dictionary: bool) -> Vec<Bridge> {
    let mut bridges = Vec::new();
    if applescript_enabled || has_scripting_dictionary {
        bridges.push(Bridge::AppleScript);
    }
    bridges.push(Bridge::Jxa);
    bridges
}

/// The name people see: the display name, else the bundle name, else the file name without `.app`. Empty names do
/// not count.
pub fn app_name(display_name: Option<&str>, bundle_name: Option<&str>, file_stem: &str) -> String {
    [display_name, bundle_name]
        .into_iter()
        .flatten()
        .find(|name| !name.is_empty())
        .unwrap_or(file_stem)
        .to_string()
}

/// The stable id: the bundle identifier, else the file name without `.app`.
pub fn app_id(bundle_identifier: Option<&str>, file_stem: &str) -> String {
    bundle_identifier
        .filter(|id| !id.is_empty())
        .unwrap_or(file_stem)
        .to_string()
}

/// Sorts apps by name (then id) and keeps the first of each id, so an app in two folders is listed once.
pub fn sort_and_dedupe(mut apps: Vec<AppInfo>) -> Vec<AppInfo> {
    apps.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    let mut seen = std::collections::BTreeSet::new();
    apps.retain(|app| seen.insert(app.id.clone()));
    apps
}

/// The app with this name, the one `launch` and `ui_tree` take.
/// The AppleScript that runs `verb` (`activate`, `quit`) on the app `name` only when it is running, so a
/// focus or quit never launches an app. The name is quoted as an AppleScript string.
pub fn running_app_script(name: &str, verb: &str) -> String {
    let quoted = applescript_string(name);
    format!("if application {quoted} is running then\ntell application {quoted} to {verb}\nend if")
}

/// `text` as an AppleScript string literal. Names with a line break or other control character are refused by the
/// caller, so only the quote and the backslash need escaping here.
pub fn applescript_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn find_app<'a>(apps: &'a [AppInfo], name: &str) -> Option<&'a AppInfo> {
    apps.iter().find(|app| app.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jxa_is_always_offered_and_applescript_needs_the_app_to_allow_it() {
        assert_eq!(scripting_bridges(false, false), vec![Bridge::Jxa]);
        assert_eq!(
            scripting_bridges(true, false),
            vec![Bridge::AppleScript, Bridge::Jxa]
        );
        assert_eq!(
            scripting_bridges(false, true),
            vec![Bridge::AppleScript, Bridge::Jxa]
        );
    }

    #[test]
    fn a_focus_or_quit_only_runs_on_a_running_app() {
        assert_eq!(
            running_app_script("Notes", "activate"),
            "if application \"Notes\" is running then\ntell application \"Notes\" to activate\nend if"
        );
    }

    #[test]
    fn names_are_quoted_so_they_cannot_end_the_string() {
        assert_eq!(applescript_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
