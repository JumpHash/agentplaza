//! The JSON layer over apz-theme, built and tested on the native target. Every function takes its inputs as JSON
//! text (a theme file, a VS Code theme) or bytes (a `.vsix`) and returns JSON text. A refusal is `{"error": "..."}`,
//! so a caller reads one shape whatever happened.

use apz_theme::backdrop;
use apz_theme::color::{Rgba, fix_contrast};
use apz_theme::generate::theme_css;
use apz_theme::skin::{self, Skin};
use apz_theme::tokens::CHECKS;
use apz_theme::{Background, Mode, Resolved, ThemeFile, Tokens, vscode, vsix};
use serde_json::{Value, json};

/// Resolves `file_json` for the skin `skin_id` and the mode `mode` (`light` or `dark`). Returns
/// `{"resolved": Resolved, "css": string}`, where the CSS sets the palette under
/// `:root[data-palette="<id>"][data-mode="<mode>"]`.
pub fn resolve(file_json: &str, skin_id: &str, mode: &str) -> String {
    reply(resolve_value(file_json, skin_id, mode))
}

/// Checks a theme file. Returns `{"ok": bool, "problems": [string]}`; text that does not parse as a theme file is
/// one of the problems.
pub fn validate(file_json: &str) -> String {
    let problems = match serde_json::from_str::<ThemeFile>(file_json) {
        Ok(file) => file.problems(),
        Err(err) => vec![format!("not a theme file: {err}")],
    };
    json!({"ok": problems.is_empty(), "problems": problems}).to_string()
}

/// The contrast rule of every check in [`CHECKS`], measured on the resolved tokens. Each row names the ground the
/// colour is closest to, and whether it reaches the rule's minimum there.
pub fn contrast(file_json: &str, skin_id: &str, mode: &str) -> String {
    reply(contrast_value(file_json, skin_id, mode))
}

/// Sets `token` in the palette of the mode the file resolves in to the nearest colour that passes its contrast
/// rule. Returns the theme file as JSON.
pub fn fix(file_json: &str, skin_id: &str, mode: &str, token: &str) -> String {
    reply(fix_value(file_json, skin_id, mode, token))
}

/// Imports a VS Code theme from its JSON text, with no include chain to follow.
pub fn import_vscode(text: &str, skin_id: &str) -> String {
    reply(import_vscode_value(text, skin_id))
}

/// The colour themes in a `.vsix` archive, as `{label, ui_theme, path}` rows.
pub fn vsix_themes(bytes: &[u8]) -> String {
    reply(vsix_themes_value(bytes))
}

/// Imports one theme from a `.vsix` archive. `pick` names the theme's path when the archive has several.
pub fn import_vsix(bytes: &[u8], pick: Option<&str>, skin_id: &str) -> String {
    reply(vsix::import(bytes, pick, skin_id).and_then(|import| import_output(import, skin_id)))
}

/// The backdrop's uniform block for a background spec (JSON) on a token map (JSON: token name to colour text), as 40
/// floats in the shader's order. `width` and `height` are the target in device px, `scale` is device px per css px,
/// `time` is in seconds, and the pointer is in css px, read when `has_pointer` is set. A refusal is its message.
pub fn backdrop_uniforms(
    bg_json: &str,
    tokens_json: &str,
    width: f32,
    height: f32,
    scale: f32,
    time: f32,
    px: f32,
    py: f32,
    has_pointer: bool,
) -> Result<Vec<f32>, String> {
    let bg: Background =
        serde_json::from_str(bg_json).map_err(|err| format!("not a background: {err}"))?;
    let tokens: Tokens =
        serde_json::from_str(tokens_json).map_err(|err| format!("not a token map: {err}"))?;
    let pointer = has_pointer.then_some((px, py));
    let uniforms = backdrop::uniforms(&bg, &tokens, width, height, scale, time, pointer);
    Ok(uniforms.floats().to_vec())
}

/// Whether a background spec (JSON) moves, so that a host draws it on a timer. A refusal is its message.
pub fn backdrop_animates(bg_json: &str) -> Result<bool, String> {
    let bg: Background =
        serde_json::from_str(bg_json).map_err(|err| format!("not a background: {err}"))?;
    Ok(backdrop::animates(&bg))
}

fn reply(result: Result<Value, String>) -> String {
    match result {
        Ok(value) => value.to_string(),
        Err(message) => json!({"error": message}).to_string(),
    }
}

fn find_skin(id: &str) -> Result<Skin, String> {
    skin::skin(id).ok_or_else(|| format!("unknown skin {id:?}"))
}

fn parse_mode(text: &str) -> Result<Mode, String> {
    match text {
        "light" => Ok(Mode::Light),
        "dark" => Ok(Mode::Dark),
        other => Err(format!("mode must be \"light\" or \"dark\", not {other:?}")),
    }
}

/// Parses a theme file (refusing one with problems) and resolves it.
fn resolve_file(file_json: &str, skin_id: &str, mode: &str) -> Result<Resolved, String> {
    let file = ThemeFile::parse(file_json)?;
    resolve_parsed(&file, skin_id, mode)
}

fn resolve_parsed(file: &ThemeFile, skin_id: &str, mode: &str) -> Result<Resolved, String> {
    let skin = find_skin(skin_id)?;
    Ok(apz_theme::resolve(file, &skin, parse_mode(mode)?))
}

fn resolve_value(file_json: &str, skin_id: &str, mode: &str) -> Result<Value, String> {
    let resolved = resolve_file(file_json, skin_id, mode)?;
    let selector = format!(
        ":root[data-palette=\"{}\"][data-mode=\"{}\"]",
        resolved.id,
        resolved.mode.as_str()
    );
    let css = theme_css(&selector, &resolved);
    Ok(json!({"resolved": resolved, "css": css}))
}

/// The ground a colour is closest to among `grounds`, and its contrast ratio there.
fn worst_ground(fg: Rgba, grounds: &[&str], resolved: &Resolved) -> (String, f32) {
    grounds
        .iter()
        .map(|g| ((*g).to_string(), fg.contrast(resolved.tokens.get(g))))
        .fold((String::new(), f32::INFINITY), |a, b| {
            if b.1 < a.1 { b } else { a }
        })
}

fn contrast_value(file_json: &str, skin_id: &str, mode: &str) -> Result<Value, String> {
    let resolved = resolve_file(file_json, skin_id, mode)?;
    let rows: Vec<Value> = CHECKS
        .iter()
        .map(|check| {
            let fg = resolved.tokens.get(check.fg).quantized();
            let (against, ratio) = worst_ground(fg, check.on, &resolved);
            json!({
                "token": check.fg,
                "against": against,
                "ratio": ratio,
                "min": check.min,
                "ok": ratio >= check.min,
            })
        })
        .collect();
    Ok(Value::Array(rows))
}

fn fix_value(file_json: &str, skin_id: &str, mode: &str, token: &str) -> Result<Value, String> {
    let mut file = ThemeFile::parse(file_json)?;
    let resolved = resolve_parsed(&file, skin_id, mode)?;
    let check = CHECKS
        .iter()
        .find(|check| check.fg == token)
        .ok_or_else(|| format!("{token:?} has no contrast rule, so there is nothing to fix"))?;
    let fg = resolved.tokens.get(check.fg);
    let grounds: Vec<Rgba> = check.on.iter().map(|g| resolved.tokens.get(g)).collect();
    let colour = fix_contrast(fg, &grounds, check.min).unwrap_or(fg);
    let palette = match resolved.mode {
        Mode::Light => file.palette.light.get_or_insert_with(Default::default),
        Mode::Dark => file.palette.dark.get_or_insert_with(Default::default),
    };
    palette.insert(check.fg.to_string(), colour.to_hex());
    serde_json::to_value(&file).map_err(|err| err.to_string())
}

fn import_vscode_value(text: &str, skin_id: &str) -> Result<Value, String> {
    let no_includes = |_: &str| -> Option<String> { None };
    let import = vscode::from_json(text, skin_id, &no_includes)?;
    import_output(import, skin_id)
}

fn vsix_themes_value(bytes: &[u8]) -> Result<Value, String> {
    let themes = vsix::themes(bytes)?;
    serde_json::to_value(&themes).map_err(|err| err.to_string())
}

/// The shape both importers return: the theme file, its name and mode, and the report of resolving it for `skin_id`.
fn import_output(import: vscode::Import, skin_id: &str) -> Result<Value, String> {
    let skin = find_skin(skin_id)?;
    let resolved = apz_theme::resolve(&import.file, &skin, import.mode);
    let mut report = serde_json::to_value(&resolved.report).map_err(|err| err.to_string())?;
    report["summary"] = json!(resolved.report.summary());
    Ok(json!({
        "file": import.file,
        "name": import.name,
        "mode": import.mode,
        "report": report,
    }))
}

#[cfg(test)]
mod tests {
    use apz_theme::builtin::{DEFAULT_THEME, builtin};

    use super::*;

    fn default_file_json() -> String {
        serde_json::to_string(&builtin(DEFAULT_THEME).expect("the default theme")).expect("JSON")
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("JSON")
    }

    #[test]
    fn resolve_gives_the_tokens_and_the_css_for_the_palette() {
        let out = parse(&resolve(&default_file_json(), "loom", "light"));
        assert!(out.get("error").is_none(), "{out}");
        assert_eq!(out["resolved"]["mode"], "light");
        let css = out["css"].as_str().expect("css");
        assert!(css.contains(r#":root[data-palette="loom-clay"][data-mode="light"]"#));
        assert!(css.contains("--paper:"));
    }

    #[test]
    fn a_refusal_comes_back_as_an_error_object() {
        let out = parse(&resolve("not json", "loom", "light"));
        assert!(
            out["error"]
                .as_str()
                .expect("error")
                .starts_with("not a theme file")
        );
        let out = parse(&resolve(&default_file_json(), "no-such-skin", "light"));
        assert!(
            out["error"]
                .as_str()
                .expect("error")
                .contains("unknown skin")
        );
        let out = parse(&resolve(&default_file_json(), "loom", "sepia"));
        assert!(out["error"].is_string());
    }

    #[test]
    fn validate_names_what_is_wrong() {
        let ok = parse(&validate(&default_file_json()));
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["problems"], json!([]));

        let mut file = builtin(DEFAULT_THEME).expect("the default theme");
        file.schema = 2;
        let bad = parse(&validate(&serde_json::to_string(&file).expect("JSON")));
        assert_eq!(bad["ok"], false);
        assert!(
            bad["problems"][0]
                .as_str()
                .expect("problem")
                .contains("schema 2")
        );

        let not_a_file = parse(&validate("{"));
        assert_eq!(not_a_file["ok"], false);
        assert!(
            not_a_file["problems"][0]
                .as_str()
                .expect("problem")
                .starts_with("not a theme file")
        );
    }

    #[test]
    fn a_builtin_passes_every_contrast_check_in_both_modes() {
        for mode in ["light", "dark"] {
            let rows = parse(&contrast(&default_file_json(), "loom", mode));
            let rows = rows.as_array().expect("rows");
            assert_eq!(rows.len(), CHECKS.len());
            for row in rows {
                assert_eq!(row["ok"], true, "{mode}: {row}");
            }
        }
    }

    #[test]
    fn fix_sets_a_low_contrast_ink_to_a_colour_that_passes() {
        let mut file = builtin(DEFAULT_THEME).expect("the default theme");
        file.palette
            .light
            .get_or_insert_with(Default::default)
            .insert("ink-3".into(), "#e8e8e8".into());
        let before = serde_json::to_string(&file).expect("JSON");
        let adjusted_before = resolve_file(&before, "loom", "light").expect("resolves");
        assert!(
            adjusted_before
                .report
                .adjusted
                .iter()
                .any(|a| a.token == "ink-3")
        );

        let out = parse(&fix(&before, "loom", "light", "ink-3"));
        assert!(out.get("error").is_none(), "{out}");
        let fixed: ThemeFile = serde_json::from_value(out).expect("a theme file");
        assert!(
            fixed
                .palette
                .light
                .as_ref()
                .expect("light")
                .contains_key("ink-3")
        );
        let after = resolve_file(
            &serde_json::to_string(&fixed).expect("JSON"),
            "loom",
            "light",
        )
        .expect("resolves");
        assert!(after.report.adjusted.iter().all(|a| a.token != "ink-3"));
        assert!(after.report.failing.iter().all(|a| a.token != "ink-3"));
    }

    #[test]
    fn fix_refuses_a_token_without_a_rule() {
        let out = parse(&fix(&default_file_json(), "loom", "light", "paper"));
        assert!(
            out["error"]
                .as_str()
                .expect("error")
                .contains("no contrast rule")
        );
    }

    #[test]
    fn backdrop_uniforms_are_the_shader_block_for_the_spec() {
        let file = apz_theme::builtin::builtin(apz_theme::builtin::DEFAULT_THEME)
            .expect("the default theme");
        let skin = apz_theme::skin::skin("loom").expect("the loom skin");
        let tokens = apz_theme::resolve(&file, &skin, Mode::Light).tokens;
        let bg = apz_theme::background::preset_by_id("loom-paper").expect("the loom-paper preset");
        let tokens_json = serde_json::to_string(&tokens).expect("JSON");
        let bg_json = serde_json::to_string(&bg).expect("JSON");

        let got = backdrop_uniforms(
            &bg_json,
            &tokens_json,
            64.0,
            48.0,
            2.0,
            1.5,
            10.0,
            20.0,
            true,
        )
        .expect("uniforms");
        let want = backdrop::uniforms(&bg, &tokens, 64.0, 48.0, 2.0, 1.5, Some((10.0, 20.0)));
        assert_eq!(got, want.floats().to_vec());
        assert_eq!(got.len(), 40);
        assert_eq!(&got[0..4], &[64.0, 48.0, 2.0, 1.5]);

        assert!(backdrop_animates(&bg_json).expect("animates"));
        assert!(!backdrop_animates("{}").expect("animates"));
        assert!(
            backdrop_uniforms(
                "not json",
                &tokens_json,
                1.0,
                1.0,
                1.0,
                0.0,
                0.0,
                0.0,
                false
            )
            .is_err()
        );
    }
}
