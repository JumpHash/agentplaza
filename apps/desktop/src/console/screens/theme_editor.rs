//! The theme editor: one theme edited token by token, with a preview drawn from its resolved palette, saved to the hub
//! and exported as `.apztheme.json`. A copy starts from a built-in's resolved colours. The import picker reads a theme
//! file, a VS Code theme (`.json`) or a `.vsix` package, shows the report and the change against the look in use, and
//! applies it or opens it in the editor first. The files and rules are `apz_theme`'s, the ones the web editor uses.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::text::LineHeight;
use iced::widget::{Space, button, column, container, pick_list, row, text, text_input};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};

use apz_theme::appearance::Document;
use apz_theme::tokens::{self, Group, Token};
use apz_theme::{
    Mode, PaletteText, Palettes, Resolved, Rgba, SCHEMA, Skin, ThemeFile, builtin, vscode, vsix,
};

use super::Id;
use crate::console::{Ctx, boot, client, ui};
use crate::theme;
use crate::theme::{
    READING, SANS, SANS_MEDIUM, SANS_SEMIBOLD, TYPE_BADGE, TYPE_CODE, TYPE_META, TYPE_PROSE,
    TYPE_UI,
};

/// The screen's target for an import: the file picker, with nothing to edit yet.
pub const IMPORT: &str = "import";

/// The file picker's source: a theme file, or a VS Code theme or package, which decides the filter the picker shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A `.apztheme.json` file.
    Theme,
    /// A VS Code theme `.json`, or a `.vsix` package.
    VsCode,
}

/// A picked file, read and checked: one theme ready to compare, or a package whose themes the owner picks from.
#[derive(Debug, Clone)]
pub enum Found {
    /// One theme, its mode, and the report's one-line summary.
    One {
        file: ThemeFile,
        mode: Mode,
        summary: String,
    },
    /// A package with several themes: its bytes are kept until the owner picks one by its label.
    Many { bytes: Vec<u8>, labels: Vec<String> },
}

/// What the screen holds: the theme being edited, the import waiting for a decision, and the last messages.
#[derive(Debug, Default)]
pub struct State {
    /// The target the screen was opened for; a different target starts over.
    target: String,
    /// The theme being edited, or None while the import picker has nothing to edit.
    file: Option<ThemeFile>,
    /// Whether the rows show the light palette (false: the dark one).
    light: bool,
    /// The hex text the owner is typing, per token; a valid value also updates the palette.
    hex: BTreeMap<String, String>,
    /// The hub's saved themes as last read.
    saved: Vec<ThemeFile>,
    /// A picked file that is not applied yet.
    pending: Option<Found>,
    loading: bool,
    saving: bool,
    error: Option<String>,
    notice: Option<String>,
    /// The skin a VS Code theme is imported with: the skin of the theme in use.
    import_skin: String,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Loaded(Result<Document, String>),
    Saved(Result<Document, String>),
    /// The name field changed.
    Name(String),
    /// A skin, by its id.
    Skin(String),
    /// The light or dark palette is shown.
    Mode(Mode),
    /// A token's hex text changed: its name and the text typed.
    Hex(String, String),
    /// Moves a failing token to the nearest colour that passes its checks.
    Fix(String),
    Save,
    Export,
    ExportPicked(Option<(PathBuf, String)>),
    Choose(Source),
    /// The skin a VS Code theme is imported with.
    ImportSkin(String),
    Picked(Option<PathBuf>),
    Parsed(Result<Found, String>),
    /// One of a package's themes, by its label.
    PickLabel(String),
    /// Saves the pending import and chooses it.
    Apply,
    /// Opens the pending import in the editor, without saving it.
    EditFirst,
    /// Drops the pending import.
    Discard,
    /// Returns to the Appearance screen.
    Close,
}

/// The navigation a screen's message asks for: Close returns to the Appearance screen.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Close => Some(Id::Appearance),
        _ => None,
    }
}

/// A skin by its id, or the default skin when the id is unknown.
fn skin_of(id: &str) -> apz_theme::Skin {
    apz_theme::skin::skin(id)
        .or_else(|| apz_theme::skin::skin(apz_theme::skin::DEFAULT_SKIN))
        .or_else(|| apz_theme::skin::skins().into_iter().next())
        .expect("apz-theme ships its built-in skins")
}

/// The skin of the theme in use: the import of a VS Code theme starts with it, as the web does.
fn active_skin(saved: &[ThemeFile]) -> String {
    let id = boot::saved_setting("appearance")
        .and_then(|value| serde_json::from_value::<Document>(value).ok())
        .map(|doc| doc.choice.theme)
        .unwrap_or_default();
    saved
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .or_else(|| builtin::builtin(&id))
        .map(|t| t.skin)
        .unwrap_or_else(|| apz_theme::skin::DEFAULT_SKIN.to_string())
}

/// The saved themes the cache holds (the hub's last answer), empty when none is saved.
fn cached_themes() -> Vec<ThemeFile> {
    boot::saved_setting("appearance")
        .and_then(|value| serde_json::from_value::<Document>(value).ok())
        .map(|doc| doc.themes)
        .unwrap_or_default()
}

/// Caches the hub's document, as the Appearance screen does, so the next start applies the same look.
fn cache(doc: &Document) {
    // Remembering is a convenience: a failed write must not block the editor.
    if let Ok(value) = serde_json::to_value(doc) {
        let _ = boot::save_setting("appearance", value);
    }
}

/// The mode the rows show.
fn mode_of(st: &State) -> Mode {
    if st.light { Mode::Light } else { Mode::Dark }
}

/// A new theme id for `name`: `custom-`, the name's letters and digits, and four random characters, so a copy never
/// takes a saved or built-in id.
fn fresh_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c == ' ' || c == '-' || c == '_' {
                Some('-')
            } else {
                None
            }
        })
        .take(24)
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "theme" } else { slug };
    let mut n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut tail = String::new();
    for _ in 0..4 {
        tail.push(LETTERS[(n % 36) as usize] as char);
        n /= 36;
    }
    format!("custom-{slug}-{tail}")
}

/// A theme's resolved colours for `mode`, as the hex text a palette holds.
fn resolved_text(file: &ThemeFile, mode: Mode) -> PaletteText {
    let r = apz_theme::resolve(file, &skin_of(&file.skin), mode);
    r.tokens
        .0
        .iter()
        .map(|(name, c)| (name.clone(), c.to_hex()))
        .collect()
}

/// A new copy of `src`: its resolved light and dark colours under a fresh id, so the copy stands on its own.
fn copy_of(src: &ThemeFile) -> ThemeFile {
    let name: String = src.name.chars().take(70).collect();
    ThemeFile {
        schema: SCHEMA,
        id: fresh_id(&src.name),
        name: format!("{name} (copy)"),
        about: src.about.clone(),
        skin: src.skin.clone(),
        palette: Palettes {
            light: Some(resolved_text(src, Mode::Light)),
            dark: Some(resolved_text(src, Mode::Dark)),
        },
        background: src.background.clone(),
        overrides: Palettes::default(),
        source: Some(format!("copy of {}", src.id)),
    }
}

/// The file a target opens on: a copy (`new:<id>`), a saved theme, or a built-in copied for editing. The import
/// picker has none.
fn start_file(target: &str, saved: &[ThemeFile]) -> Option<ThemeFile> {
    let lookup = |id: &str| saved.iter().find(|t| t.id == id).cloned();
    match target.strip_prefix("new:") {
        Some(source) => lookup(source)
            .or_else(|| builtin::builtin(source))
            .map(|src| copy_of(&src)),
        None if target == IMPORT => None,
        None => lookup(target).or_else(|| builtin::builtin(target).map(|src| copy_of(&src))),
    }
}

/// The colours of `mode` in a file, made from what the file resolves to in that mode when it has none for it.
fn mode_text(file: &mut ThemeFile, mode: Mode) -> &mut PaletteText {
    if file.palette.get(mode).is_none() {
        let text = resolved_text(file, mode);
        match mode {
            Mode::Light => file.palette.light = Some(text),
            Mode::Dark => file.palette.dark = Some(text),
        }
    }
    match mode {
        Mode::Light => file.palette.light.get_or_insert_with(PaletteText::new),
        Mode::Dark => file.palette.dark.get_or_insert_with(PaletteText::new),
    }
}

/// The lowest contrast ratio of a token against the grounds of each check that names it, with the minimum that check
/// needs. The check nearest to failing is returned; None when no check names the token.
fn contrast_of(name: &str, r: &Resolved) -> Option<(f32, f32)> {
    let fg = r.tokens.get(name).quantized();
    tokens::CHECKS
        .iter()
        .filter(|check| check.fg == name)
        .map(|check| {
            let ratio = check
                .on
                .iter()
                .map(|g| fg.contrast(r.tokens.get(g)))
                .fold(f32::INFINITY, f32::min);
            (ratio, check.min)
        })
        .min_by(|a, b| {
            (a.0 / a.1)
                .partial_cmp(&(b.0 / b.1))
                .unwrap_or(Ordering::Equal)
        })
}

/// Moves a token, in the mode's palette, to the nearest colour that passes each of its checks.
fn fix(file: &mut ThemeFile, name: &str, mode: Mode) {
    let r = apz_theme::resolve(file, &skin_of(&file.skin), mode);
    let mut fg = r.tokens.get(name);
    for check in tokens::CHECKS.iter().filter(|check| check.fg == name) {
        let grounds: Vec<Rgba> = check.on.iter().map(|g| r.tokens.get(g)).collect();
        if let Some(to) = apz_theme::color::fix_contrast(fg, &grounds, check.min) {
            fg = to;
        }
    }
    mode_text(file, mode).insert(name.to_string(), fg.to_hex());
}

/// A found theme for a file: a built-in id is replaced by a fresh one, so an import never shadows a built-in.
fn found(mut file: ThemeFile, mode: Mode) -> Found {
    if builtin::is_builtin(&file.id) {
        file.id = fresh_id(&file.name);
    }
    let r = apz_theme::resolve(&file, &skin_of(&file.skin), mode);
    let mut summary = r.report.summary();
    if !r.report.failing.is_empty() {
        summary.push_str(&format!(
            "; {} still below the contrast minimum",
            r.report.failing.len()
        ));
    }
    Found::One {
        file,
        mode,
        summary,
    }
}

/// Reads a picked file and checks it: a theme file, a VS Code theme (`.json`), or a `.vsix` package whose themes the
/// owner picks from when it has several.
fn read_found(path: &Path, skin: &str) -> Result<Found, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("The file could not be read: {e}"))?;
    let is_vsix = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vsix"));
    if is_vsix {
        let labels: Vec<String> = vsix::themes(&bytes)?.into_iter().map(|t| t.label).collect();
        return match labels.len() {
            0 => Err("The package contributes no themes.".to_string()),
            1 => {
                let imp = vsix::import(&bytes, Some(labels[0].as_str()), skin)?;
                Ok(found(imp.file, imp.mode))
            }
            _ => Ok(Found::Many { bytes, labels }),
        };
    }
    let text = String::from_utf8(bytes).map_err(|_| "The file is not text.".to_string())?;
    match ThemeFile::parse(&text) {
        Ok(file) => {
            let mode = if file.palette.light.is_some() {
                Mode::Light
            } else {
                Mode::Dark
            };
            Ok(found(file, mode))
        }
        Err(theme_error) => {
            match vscode::from_json(&text, skin, &|_: &str| -> Option<String> { None }) {
                Ok(imp) => Ok(found(imp.file, imp.mode)),
                Err(e) => Err(format!(
                    "This is not a theme file ({theme_error}) or a VS Code theme ({e})."
                )),
            }
        }
    }
}

/// Saves a theme to the hub and chooses it: the theme is written, then the look names it, and the hub answers with its
/// document.
fn save(ctx: &Ctx, file: ThemeFile) -> Task<Msg> {
    let look = super::appearance::choose_theme(&file.id);
    let id = file.id.clone();
    let c = ctx.client.clone();
    client::task(
        move || {
            let body = serde_json::to_value(&file).map_err(|e| e.to_string())?;
            let _: Document = c.put(&format!("/v1/appearance/themes/{id}"), &body)?;
            let look_body = serde_json::to_value(&look).map_err(|e| e.to_string())?;
            c.put::<Document>("/v1/appearance", &look_body)
        },
        Msg::Saved,
    )
}

pub fn load(st: &mut State, target: &str, ctx: &Ctx) -> Task<Msg> {
    if st.target != target {
        *st = State {
            target: target.to_string(),
            light: theme::is_light(),
            ..State::default()
        };
        st.saved = cached_themes();
        st.import_skin = active_skin(&st.saved);
        st.file = start_file(target, &st.saved);
    }
    st.loading = true;
    let c = ctx.client.clone();
    client::task(move || c.get::<Document>("/v1/appearance"), Msg::Loaded)
}

pub fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Loaded(res) => {
            st.loading = false;
            match res {
                Ok(doc) => {
                    st.saved = doc.themes;
                    if st.file.is_none() && st.pending.is_none() {
                        st.file = start_file(&st.target, &st.saved);
                    }
                }
                Err(e) => st.error = Some(e),
            }
            Task::none()
        }
        Msg::Saved(res) => {
            st.saving = false;
            match res {
                Ok(doc) => {
                    cache(&doc);
                    // This device's own look (already named with the theme) wins over the hub's choice on this device.
                    let choice =
                        super::appearance::device_look().unwrap_or_else(|| doc.choice.clone());
                    theme::apply(&choice, &doc.themes);
                    st.saved = doc.themes;
                    st.error = None;
                    st.notice =
                        Some("Saved. This look is applied on this device and on the hub.".into());
                }
                Err(e) => st.error = Some(format!("The theme was not saved to the hub: {e}")),
            }
            Task::none()
        }
        Msg::Name(name) => {
            if let Some(file) = st.file.as_mut() {
                file.name = name;
            }
            Task::none()
        }
        Msg::Skin(id) => {
            if let Some(file) = st.file.as_mut() {
                file.skin = id;
            }
            Task::none()
        }
        Msg::Mode(mode) => {
            st.light = mode == Mode::Light;
            st.hex.clear();
            if let Some(file) = st.file.as_mut() {
                mode_text(file, mode);
            }
            Task::none()
        }
        Msg::Hex(name, text) => {
            let mode = mode_of(st);
            if let (Some(file), Some(c)) = (st.file.as_mut(), Rgba::parse(&text)) {
                mode_text(file, mode).insert(name.clone(), c.to_hex());
            }
            st.hex.insert(name, text);
            Task::none()
        }
        Msg::Fix(name) => {
            let mode = mode_of(st);
            if let Some(file) = st.file.as_mut() {
                fix(file, &name, mode);
            }
            st.hex.remove(&name);
            Task::none()
        }
        Msg::Save => {
            if st.saving {
                return Task::none();
            }
            let Some(file) = st.file.clone() else {
                return Task::none();
            };
            let problems = file.problems();
            if !problems.is_empty() {
                st.error = Some(problems.join("; "));
                return Task::none();
            }
            st.error = None;
            st.notice = None;
            st.saving = true;
            save(ctx, file)
        }
        Msg::Export => {
            let Some(file) = st.file.clone() else {
                return Task::none();
            };
            match serde_json::to_string_pretty(&file) {
                Ok(text) => {
                    let name = format!("{}.apztheme.json", file.id);
                    Task::perform(
                        async move {
                            rfd::AsyncFileDialog::new()
                                .set_title("Export the theme")
                                .set_file_name(name)
                                .add_filter("Theme file", &["json"])
                                .save_file()
                                .await
                                .map(|picked| (picked.path().to_path_buf(), text))
                        },
                        Msg::ExportPicked,
                    )
                }
                Err(e) => {
                    st.error = Some(format!("The theme could not be exported: {e}"));
                    Task::none()
                }
            }
        }
        Msg::ExportPicked(None) => Task::none(),
        Msg::ExportPicked(Some((path, text))) => {
            st.error = None;
            match std::fs::write(&path, text) {
                Ok(()) => st.notice = Some(format!("Exported to {}.", path.display())),
                Err(e) => st.error = Some(format!("The file could not be written: {e}")),
            }
            Task::none()
        }
        Msg::Choose(source) => Task::perform(
            async move {
                let dialog = match source {
                    Source::Theme => rfd::AsyncFileDialog::new()
                        .set_title("Choose a theme file")
                        .add_filter("Theme files", &["json"]),
                    Source::VsCode => rfd::AsyncFileDialog::new()
                        .set_title("Choose a VS Code theme")
                        .add_filter("VS Code themes", &["json", "vsix"]),
                };
                dialog
                    .pick_file()
                    .await
                    .map(|picked| picked.path().to_path_buf())
            },
            Msg::Picked,
        ),
        Msg::Picked(None) => Task::none(),
        Msg::Picked(Some(path)) => {
            st.error = None;
            st.notice = None;
            let skin = st.import_skin.clone();
            client::task(move || read_found(&path, &skin), Msg::Parsed)
        }
        Msg::Parsed(Err(e)) => {
            st.error = Some(e);
            Task::none()
        }
        Msg::Parsed(Ok(found)) => {
            st.error = None;
            st.pending = Some(found);
            Task::none()
        }
        Msg::PickLabel(label) => {
            let Some(Found::Many { bytes, .. }) = st.pending.clone() else {
                return Task::none();
            };
            let skin = st.import_skin.clone();
            client::task(
                move || {
                    let imp = vsix::import(&bytes, Some(&label), &skin)?;
                    Ok(found(imp.file, imp.mode))
                },
                Msg::Parsed,
            )
        }
        Msg::Apply => {
            let Some(Found::One { file, .. }) = st.pending.clone() else {
                return Task::none();
            };
            st.pending = None;
            st.error = None;
            st.notice = None;
            st.saving = true;
            save(ctx, file)
        }
        Msg::EditFirst => {
            let Some(Found::One { file, mode, .. }) = st.pending.clone() else {
                return Task::none();
            };
            st.pending = None;
            st.light = mode == Mode::Light;
            st.hex.clear();
            st.file = Some(file);
            // The editor is a page of its own now: a later visit to the import starts over.
            st.target = format!("edit:{}", st.file.as_ref().map_or("", |f| f.id.as_str()));
            st.error = None;
            Task::none()
        }
        Msg::ImportSkin(id) => {
            st.import_skin = id;
            Task::none()
        }
        Msg::Discard => {
            st.pending = None;
            Task::none()
        }
        Msg::Close => Task::none(),
    }
}

/// A small label in a chip: its text on its soft ground, as the web's `.chip.t-<tone>`.
fn chip<'a>(label: String, fg: Color, ground: Color) -> Element<'a, Msg> {
    container(text(label).font(SANS_MEDIUM).size(TYPE_BADGE).color(fg))
        .padding([3, 6])
        .style(move |_: &Theme| container::Style {
            background: Some(ground.into()),
            border: Border::default().rounded(6.0),
            ..Default::default()
        })
        .into()
}

/// A labelled control in a column: the web's `.field`.
fn field<'a>(label: &str, control: Element<'a, Msg>) -> Element<'a, Msg> {
    column![
        text(label.to_string())
            .font(SANS_MEDIUM)
            .size(TYPE_UI)
            .color(theme::palette().ink_2),
        control,
    ]
    .spacing(5)
    .width(Length::Fill)
    .into()
}

/// The page head, as the web's `.page-head`: the trail above the title, the actions at the title's right, and the
/// lede under it.
fn page_head<'a>(
    trail: &str,
    title: String,
    actions: Vec<Element<'a, Msg>>,
    lede: &str,
) -> Element<'a, Msg> {
    let p = theme::palette();
    let kicker = row![
        button(text("Appearance").font(SANS).size(TYPE_UI).color(p.ink_3))
            .padding(0)
            .style(button::text)
            .on_press(Msg::Close),
        text("/").font(SANS).size(TYPE_UI).color(p.ink_3),
        text(trail.to_string())
            .font(SANS)
            .size(TYPE_UI)
            .color(p.ink_3),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let title_row = row![
        text(title)
            .font(SANS_SEMIBOLD)
            .size(ui::TYPE_H1)
            .line_height(LineHeight::Relative(1.25))
            .color(p.ink),
        Space::new().width(Length::Fill),
        row(actions).spacing(8).align_y(Alignment::Center),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let lede = text(lede.to_string())
        .font(SANS)
        .size(TYPE_PROSE)
        .line_height(READING)
        .color(p.ink_2)
        .width(Length::Fixed(620.0));
    column![kicker, title_row, lede].spacing(6).into()
}

/// A page: the head over the body, in the console's sheet and scrolled.
fn page<'a>(head: Element<'a, Msg>, body: Element<'a, Msg>) -> Element<'a, Msg> {
    ui::scroll(ui::sheet(head, body)).width(Length::Fill).into()
}

/// The status lines a screen shows under its head: the last error and the last notice.
fn status_lines<'a>(st: &State) -> Vec<Element<'a, Msg>> {
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = st.error.as_ref() {
        lines.push(ui::error_line(e));
    }
    if let Some(n) = st.notice.as_ref() {
        lines.push(ui::faint(n.clone()).into());
    }
    lines
}

/// The sample page's chip, diff and syntax colours and its card come from the theme's own palette, so a preview shows
/// what the theme would look like. `skin` gives the label voice and the corner radius.
fn sample<'a>(p: theme::Palette, skin: &Skin) -> Element<'a, Msg> {
    let radius = skin.radius;
    let radius_lg = skin.radius_lg;
    let session_head = {
        let label: Element<'a, Msg> = if skin.label.mono {
            ui::mono(
                if skin.label.upper {
                    "SESSION OVERVIEW".to_string()
                } else {
                    "Session overview".to_string()
                },
                11.0,
                p.ink_3,
            )
            .into()
        } else {
            text("Session overview")
                .font(SANS_SEMIBOLD)
                .size(12.5)
                .color(p.ink_3)
                .into()
        };
        container(
            row![
                label,
                Space::new().width(Length::Fill),
                ui::sans("Updated 2 minutes ago".to_string(), TYPE_META, p.ink_3),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([8, 12])
        .width(Length::Fill)
    };
    let approve = container(ui::sans("Approve".to_string(), 12.5, p.on_action))
        .padding([4, 9])
        .style(move |_: &Theme| container::Style {
            background: Some(p.action.into()),
            border: Border::default().rounded(radius),
            ..Default::default()
        });
    let details = container(ui::sans("Details".to_string(), 12.5, p.ink))
        .padding([4, 9])
        .style(move |_: &Theme| container::Style {
            border: Border {
                color: p.rule_strong,
                width: 1.0,
                radius: radius.into(),
            },
            ..Default::default()
        });
    let session = container(column![
        session_head,
        container(
            column![
                ui::sans(
                    "Three agents are working on the import pipeline. One waits for a review."
                        .to_string(),
                    TYPE_UI,
                    p.ink,
                ),
                row![approve, details].spacing(6),
            ]
            .spacing(8)
        )
        .padding(12)
        .width(Length::Fill),
    ])
    .width(Length::Fill)
    .style(move |_: &Theme| container::Style {
        background: Some(p.raised.into()),
        border: Border {
            color: p.rule,
            width: 1.0,
            radius: radius_lg.into(),
        },
        ..Default::default()
    });
    let chips = row![
        chip("Passing".to_string(), p.ok, p.ok_soft),
        chip("Waiting".to_string(), p.warn, p.warn_soft),
        chip("Failed".to_string(), p.crit, p.crit_soft),
        chip("Queued".to_string(), p.info, p.info_soft),
    ]
    .spacing(6);
    let diff =
        |sign: &str, number: &str, code: &str, fg: Color, ground: Color| -> Element<'a, Msg> {
            container(
                row![
                    ui::mono(number.to_string(), 12.0, p.ink_3)
                        .width(Length::Fixed(22.0))
                        .align_x(iced::alignment::Horizontal::Right),
                    ui::mono(sign.to_string(), 12.0, fg).width(Length::Fixed(14.0)),
                    ui::mono(code.to_string(), 12.0, p.ink),
                ]
                .spacing(6),
            )
            .padding([2, 6])
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(ground.into()),
                ..Default::default()
            })
            .into()
        };
    // The six sample bars, in per cent of the chart's inner height; the even ones are signal, the odd ones info.
    let heights = [62.0_f32, 84.0, 48.0, 92.0, 71.0, 56.0];
    let bars: Vec<Element<'a, Msg>> = heights
        .iter()
        .enumerate()
        .map(|(i, pct)| {
            let colour = if i % 2 == 0 { p.signal } else { p.info };
            container(
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(56.0 * pct / 100.0)),
            )
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(Color { a: 0.8, ..colour }.into()),
                border: Border {
                    radius: iced::border::Radius {
                        top_left: 3.0,
                        top_right: 3.0,
                        bottom_right: 0.0,
                        bottom_left: 0.0,
                    },
                    ..Border::default()
                },
                ..Default::default()
            })
            .into()
        })
        .collect();
    let chart = container(
        row(bars)
            .spacing(6)
            .height(Length::Fill)
            .align_y(Alignment::End),
    )
    .height(Length::Fixed(72.0))
    .padding(8)
    .width(Length::Fill)
    .style(move |_: &Theme| container::Style {
        background: Some(p.sheet.into()),
        border: Border {
            color: p.rule,
            width: 1.0,
            radius: radius.into(),
        },
        ..Default::default()
    });
    let code = container(row![
        ui::mono("1", 12.0, p.ink_3)
            .width(Length::Fixed(18.0))
            .align_x(iced::alignment::Horizontal::Right),
        ui::mono("const", 12.0, p.syn_keyword),
        ui::mono(" page = ", 12.0, p.ink),
        ui::mono("40", 12.0, p.syn_number),
        ui::mono("; ", 12.0, p.ink),
        ui::mono("// items per page", 12.0, p.syn_comment),
    ])
    .padding([4, 6])
    .width(Length::Fill)
    .style(move |_: &Theme| container::Style {
        background: Some(p.sheet_2.into()),
        ..Default::default()
    });
    container(
        column![
            session,
            chips,
            diff("+", "12", "const limit = 40;", p.add_fg, p.add_bg),
            diff("−", "12", "const limit = 20;", p.del_fg, p.del_bg),
            chart,
            code,
        ]
        .spacing(10),
    )
    .padding(12)
    .width(Length::Fill)
    .style(move |_: &Theme| container::Style {
        text_color: Some(p.ink),
        background: Some(p.paper.into()),
        border: Border {
            color: p.rule,
            width: 1.0,
            radius: radius_lg.into(),
        },
        ..Default::default()
    })
    .into()
}

/// One token's row of the table: its name and purpose, its colour (a swatch and the hex text) and its contrast, with a
/// Fix when a check fails.
fn token_row<'a>(
    token: &Token,
    r: &Resolved,
    typed: Option<&String>,
    p: &theme::Palette,
) -> Element<'a, Msg> {
    let name = token.name.to_string();
    let colour = theme::color(r.tokens.get(token.name));
    let value = typed
        .cloned()
        .unwrap_or_else(|| r.tokens.get(token.name).to_hex());
    let hex_name = name.clone();
    let input = text_input("#rrggbb", &value)
        .on_input(move |text| Msg::Hex(hex_name.clone(), text))
        .font(theme::MONO)
        .padding([5, 8])
        .size(TYPE_CODE)
        .width(Length::Fixed(104.0))
        .style(theme::input);
    let swatch = container(
        Space::new()
            .width(Length::Fixed(28.0))
            .height(Length::Fixed(28.0)),
    )
    .style(move |_: &Theme| container::Style {
        background: Some(colour.into()),
        border: Border {
            color: theme::palette().rule_strong,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    });
    let contrast: Element<'a, Msg> = match contrast_of(token.name, r) {
        Some((ratio, min)) => {
            let failing = ratio < min;
            let (fg, ground) = if failing {
                (p.crit, p.crit_soft)
            } else {
                (p.ok, p.ok_soft)
            };
            let chip = chip(format!("{ratio:.1} : 1"), fg, ground);
            if failing {
                row![
                    chip,
                    ui::btn_sm("Fix", None, ui::Kind::Ghost, Some(Msg::Fix(name.clone()))),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
            } else {
                chip
            }
        }
        None => chip(String::new(), p.ink_3, p.hover),
    };
    row![
        column![
            ui::mono(name, TYPE_CODE, p.ink),
            ui::faint(token.about.to_string()),
        ]
        .spacing(2)
        .width(Length::Fill),
        row![swatch, input]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fixed(150.0)),
        row![contrast]
            .align_y(Alignment::Center)
            .width(Length::Fixed(150.0)),
    ]
    .align_y(Alignment::Center)
    .into()
}

/// The token table of one group, as the web's `.tbl`: a head row in the label voice, each row padded and ruled under.
fn token_table<'a>(rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let p = theme::palette();
    let rule = |colour: Color| {
        container(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(colour.into()),
                ..Default::default()
            })
    };
    let head = container(row![
        ui::label("Token").width(Length::Fill),
        ui::label("Colour").width(Length::Fixed(150.0)),
        ui::label("Contrast").width(Length::Fixed(150.0)),
    ])
    .padding([8, 12])
    .width(Length::Fill);
    let mut table = column![head, rule(p.rule_strong)].width(Length::Fill);
    let count = rows.len();
    for (i, entry) in rows.into_iter().enumerate() {
        table = table.push(container(entry).padding([9, 12]).width(Length::Fill));
        if i + 1 < count {
            table = table.push(rule(p.rule));
        }
    }
    table.into()
}

/// The Colours card: the light or dark tabs, then each group's table.
fn colours_card<'a>(st: &State, resolved: &Resolved, p: &theme::Palette) -> Element<'a, Msg> {
    let mode = mode_of(st);
    let mut groups: Vec<(Group, Vec<Element<'a, Msg>>)> = Vec::new();
    for token in tokens::TOKENS {
        let entry = token_row(token, resolved, st.hex.get(token.name), p);
        match groups.iter_mut().find(|(group, _)| *group == token.group) {
            Some((_, rows)) => rows.push(entry),
            None => groups.push((token.group, vec![entry])),
        }
    }
    let mut body = column![ui::tabs(
        &[(Mode::Light, "Light"), (Mode::Dark, "Dark")],
        mode,
        Msg::Mode
    )]
    .spacing(18);
    for (group, rows) in groups {
        body = body.push(
            column![
                text(group.label().to_string())
                    .font(SANS_MEDIUM)
                    .size(TYPE_META)
                    .color(p.ink_3),
                token_table(rows),
            ]
            .spacing(6),
        );
    }
    ui::card("Colours", body.into())
}

/// The Problems card: the checks the theme fails, or the note that it can be saved.
fn problems_card<'a>(file: &ThemeFile, resolved: &Resolved) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut items = file.problems();
    let failing = resolved.report.failing.len();
    if failing > 0 {
        items.push(format!(
            "{failing} contrast {} fail; use Fix on the marked tokens.",
            if failing == 1 { "check" } else { "checks" }
        ));
    }
    let body: Element<'a, Msg> = if items.is_empty() {
        ui::badge("No problems. The theme can be saved.", ui::Tone::Ok)
    } else {
        let lines: Vec<Element<'a, Msg>> = items
            .into_iter()
            .map(|item| ui::sans(format!("• {item}"), TYPE_UI, p.ink_2).into())
            .collect();
        column(lines).spacing(4).into()
    };
    ui::card("Problems", body)
}

/// The editor: the page head with its actions; the theme's name, skin and id; the colours in both modes, and the live
/// preview and problems beside them.
fn editor_page<'a>(st: &State, file: &ThemeFile) -> Element<'a, Msg> {
    let mode = mode_of(st);
    let skin = skin_of(&file.skin);
    let resolved = apz_theme::resolve(file, &skin, mode);
    let p = theme::Palette::from_tokens(&resolved.tokens);
    let skin_ids: Vec<String> = apz_theme::skin::skins().into_iter().map(|s| s.id).collect();
    let title = if file.name.trim().is_empty() {
        "Theme".to_string()
    } else {
        file.name.clone()
    };
    let head = page_head(
        "Theme",
        title,
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Close)),
            ui::btn("Export", None, ui::Kind::Ghost, Some(Msg::Export)),
            ui::btn("Save", None, ui::Kind::Primary, Some(Msg::Save)),
        ],
        "Each colour of this theme, in both modes. The preview shows the page with the colours in force.",
    );
    let theme_card = ui::card(
        "Theme",
        column![
            row![
                field(
                    "Name",
                    text_input("Name", &file.name)
                        .on_input(Msg::Name)
                        .padding([7, 10])
                        .size(TYPE_UI)
                        .style(theme::input)
                        .width(Length::Fill)
                        .into(),
                ),
                field(
                    "Skin",
                    pick_list(skin_ids, Some(file.skin.clone()), Msg::Skin)
                        .style(crate::theme::pick_list)
                        .width(Length::Fill)
                        .into(),
                ),
            ]
            .spacing(16),
            row![
                ui::faint("ID"),
                ui::mono(file.id.clone(), TYPE_META, p.ink_3),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ]
        .spacing(10)
        .into(),
    );
    let left = column![theme_card, colours_card(st, &resolved, &p)]
        .spacing(20)
        .width(Length::Fill);
    let right = column![
        ui::card("Preview", sample(p, &skin)),
        problems_card(file, &resolved),
    ]
    .spacing(20)
    .width(Length::Fixed(360.0));
    let mut body = column![].spacing(20).width(Length::Fill);
    for line in status_lines(st) {
        body = body.push(line);
    }
    body = body.push(row![left, right].spacing(20).align_y(Alignment::Start));
    page(head, body.into())
}

/// One of the import's two sources: a drop-in button for a file of that kind.
fn source_zone<'a>(title: &str, hint: &str, source: Source) -> Element<'a, Msg> {
    let p = theme::palette();
    let body = column![
        text(title.to_string())
            .font(SANS_SEMIBOLD)
            .size(TYPE_UI)
            .color(p.ink),
        ui::faint(hint.to_string()),
    ]
    .spacing(4);
    button(body)
        .padding(18)
        .width(Length::Fill)
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(p.sheet_2.into()),
                text_color: p.ink,
                border: Border {
                    color: if hovered { p.signal } else { p.rule_strong },
                    width: 1.0,
                    radius: theme::skin().radius_lg.into(),
                },
                ..Default::default()
            }
        })
        .on_press(Msg::Choose(source))
        .into()
}

/// The import page's Source card: the skin for VS Code themes, the two sources, and a package's themes to pick from.
fn source_card<'a>(st: &State) -> Element<'a, Msg> {
    let skin_ids: Vec<String> = apz_theme::skin::skins().into_iter().map(|s| s.id).collect();
    let zones = row![
        source_zone(
            "Theme file",
            "An .apztheme.json file, from this console or another install",
            Source::Theme,
        ),
        source_zone(
            "VS Code theme",
            "A colour theme as .json, or an extension as .vsix",
            Source::VsCode,
        ),
    ]
    .spacing(12);
    let mut body = column![
        field(
            "Skin for VS Code themes",
            pick_list(skin_ids, Some(st.import_skin.clone()), Msg::ImportSkin)
                .style(crate::theme::pick_list)
                .width(Length::Fill)
                .into(),
        ),
        zones,
    ]
    .spacing(12);
    if let Some(Found::Many { labels, .. }) = st.pending.as_ref() {
        body = body.push(ui::faint(format!(
            "{} colour themes in this extension. Pick one:",
            labels.len()
        )));
        for label in labels {
            body = body.push(ui::btn(
                label,
                None,
                ui::Kind::Ghost,
                Some(Msg::PickLabel(label.clone())),
            ));
        }
    }
    ui::card("Source", body.into())
}

/// A picked theme, beside the look in use: the two samples, the note on the file, and Apply, Edit first and Discard.
fn found_card<'a>(st: &State, found: &Found) -> Option<Element<'a, Msg>> {
    let Found::One {
        file,
        mode,
        summary,
    } = found
    else {
        return None;
    };
    let note = if st.saved.iter().any(|t| t.id == file.id) {
        format!("{summary}. A saved theme with this id is replaced.")
    } else {
        summary.clone()
    };
    let imported = apz_theme::resolve(file, &skin_of(&file.skin), *mode);
    let compare = row![
        column![
            ui::faint("In use now"),
            sample(*theme::palette(), &skin_of(&theme::skin().id)),
        ]
        .spacing(6)
        .width(Length::Fill),
        column![
            ui::faint("Imported"),
            sample(
                theme::Palette::from_tokens(&imported.tokens),
                &skin_of(&file.skin)
            ),
        ]
        .spacing(6)
        .width(Length::Fill),
    ]
    .spacing(12);
    let actions = row![
        ui::btn("Apply", None, ui::Kind::Primary, Some(Msg::Apply)),
        ui::btn(
            "Edit first",
            None,
            ui::Kind::Secondary,
            Some(Msg::EditFirst)
        ),
        ui::btn("Discard", None, ui::Kind::Ghost, Some(Msg::Discard)),
    ]
    .spacing(8);
    Some(ui::card(
        &file.name,
        column![ui::faint(note), compare, actions]
            .spacing(12)
            .into(),
    ))
}

/// The import page: the head, the Source card, and the picked theme beside the look in use, once a file is picked.
fn import_page<'a>(st: &State) -> Element<'a, Msg> {
    let head = page_head(
        "Import",
        "Import a theme".to_string(),
        vec![ui::btn("Back", None, ui::Kind::Ghost, Some(Msg::Close))],
        "Choose an .apztheme.json file, or a VS Code colour theme as .json or .vsix. Nothing is saved until you apply it.",
    );
    let mut body = column![].spacing(20).width(Length::Fill);
    for line in status_lines(st) {
        body = body.push(line);
    }
    if st.loading && st.pending.is_none() {
        body = body.push(ui::loading());
    }
    body = body.push(source_card(st));
    if let Some(card) = st.pending.as_ref().and_then(|found| found_card(st, found)) {
        body = body.push(card);
    }
    page(head, body.into())
}

pub fn view(st: &State) -> Element<'_, Msg> {
    match (st.file.as_ref(), st.pending.as_ref()) {
        (Some(file), None) => editor_page(st, file),
        _ => import_page(st),
    }
}
