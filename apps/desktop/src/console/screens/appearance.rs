//! The Appearance screen: the owner's look for this hub, on the web and on the desktop. The hub keeps one choice and
//! the owner's saved themes (`GET` and `PUT /v1/appearance`, a document with `choice` and `themes`). This console
//! caches that document in desktop.json and applies it at start-up. A device may keep its own look
//! (`appearance_device`), which wins here until it is cleared, and then the hub's choice shows again.

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use iced::widget::canvas::{self, Path, Stroke};
use iced::widget::{Space, button, column, container, row, text};
use iced::{
    Alignment, Border, Color, Element, Length, Point, Rectangle, Renderer, Shadow, Subscription,
    Task, Theme, Vector, mouse,
};
use serde_json::Value;

use apz_theme::appearance::{Appearance, Density, Document, ModeChoice};
use apz_theme::background::{self, Background as BgSpec, Pattern};
use apz_theme::skin::{ActionFrom, Skin};
use apz_theme::{BackgroundChoice, Mode, Resolved, ThemeFile, builtin};

use super::Id;
use crate::console::{Ctx, boot, client, events, ui};
use crate::theme;
use crate::theme::{SANS, SANS_MEDIUM, SANS_SEMIBOLD, TYPE_ITEM, TYPE_META, TYPE_UI};

/// The code size's bounds, in px: the web's stepper stops at these.
const CODE_MIN: f32 = 11.0;
const CODE_MAX: f32 = 16.0;

/// What the screen holds: the hub's document, this device's override, and the last message to show.
#[derive(Debug, Default)]
pub struct State {
    loading: bool,
    /// The change on its way to the hub while a save is in flight. Reads that answer meanwhile are not applied, so
    /// they cannot undo it.
    sent: Option<Appearance>,
    /// The newest change the owner made that is not sent yet: it goes out when the change in flight is answered.
    queued: Option<Appearance>,
    error: Option<String>,
    notice: Option<String>,
    /// The hub's document as last confirmed by a read or a write: its choice and the owner's saved themes.
    doc: Option<Document>,
    /// This device's own look, when it keeps one; it wins over the hub's choice on this device.
    device: Option<Appearance>,
    /// The saved theme a delete is waiting to confirm: its id and name.
    confirm: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Loaded(Result<Document, String>),
    Saved(Result<Document, String>),
    /// A hub event: an appearance that changed reloads the document.
    Event(events::Event),
    /// A skin, by its id; the theme's own skin is the default.
    Skin(String),
    /// A palette, by its theme's id.
    Theme(String),
    /// A background preset, by its id; None is the theme's own background.
    Background(Option<String>),
    Mode(ModeChoice),
    Density(Density),
    CodeSize(f32),
    /// This device keeps its own look (true) or follows the hub again (false).
    DeviceOnly(bool),
    /// Edit a saved theme in the theme editor, by its id.
    OpenEditor(String),
    /// Copy a theme (built in or saved) and edit the copy, by the original's id.
    Duplicate(String),
    /// Import a theme file in the theme editor.
    Import,
    /// Import a VS Code theme or package in the theme editor.
    ImportVsCode,
    /// Ask to delete a saved theme: its id and name, confirmed in a dialog.
    AskDelete(String, String),
    CancelDelete,
    ConfirmDelete,
    /// The hub's document after a delete, or why it failed.
    Deleted(Result<Document, String>),
}

/// The navigation a screen's message asks for: the theme editor for the theme, its copy or an import.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::OpenEditor(id) => Some(Id::ThemeEditor(id.clone())),
        Msg::Duplicate(id) => Some(Id::ThemeEditor(format!("new:{id}"))),
        Msg::Import | Msg::ImportVsCode => {
            Some(Id::ThemeEditor(super::theme_editor::IMPORT.to_string()))
        }
        _ => None,
    }
}

/// `APZ_APPEARANCE=<JSON Appearance>` (a capture hook for the parity harness): the look this console shows
/// instead of the cached or hub one. It is never saved; a value that does not parse is ignored.
fn env_look() -> Option<Appearance> {
    let value = std::env::var("APZ_APPEARANCE").ok()?;
    serde_json::from_str(&value).ok()
}

/// The owner's appearance as the hub last answered, read from desktop.json.
fn read_cached() -> Option<Document> {
    boot::saved_setting("appearance").and_then(|value| serde_json::from_value(value).ok())
}

/// This device's own look, when it keeps one.
fn read_device() -> Option<Appearance> {
    boot::saved_setting("appearance_device").and_then(|value| serde_json::from_value(value).ok())
}

fn cache(doc: &Document) {
    // Remembering is a convenience: a failed write must not block the console.
    if let Ok(value) = serde_json::to_value(doc) {
        let _ = boot::save_setting("appearance", value);
    }
}

/// Keeps this device's look, or forgets it (null, which reads back as no override).
fn remember_device(device: Option<&Appearance>) {
    let value = device
        .and_then(|look| serde_json::to_value(look).ok())
        .unwrap_or(Value::Null);
    let _ = boot::save_setting("appearance_device", value);
}

/// Applies the look a console starts with, before its first frame: this device's override, else the cached hub choice,
/// with the cached themes. Returns false when neither is saved, so the old `theme` setting decides the mode instead.
pub fn startup() -> bool {
    if let Some(look) = env_look() {
        let themes = read_cached().map(|d| d.themes).unwrap_or_default();
        theme::apply(&look, &themes);
        return true;
    }
    let device = read_device();
    let doc = read_cached();
    if device.is_none() && doc.is_none() {
        return false;
    }
    let themes = doc.as_ref().map(|d| d.themes.clone()).unwrap_or_default();
    let choice = device.or_else(|| doc.map(|d| d.choice)).unwrap_or_default();
    theme::apply(&choice, &themes);
    true
}

/// Reads the hub's document as the console connects: it is cached, and applied unless this device has its own look.
pub fn sync(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(move || c.get::<Document>("/v1/appearance"), Msg::Loaded)
}

/// Reads the document again (the screen's refresh, and an appearance event).
pub fn load(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    st.loading = true;
    sync(ctx)
}

/// Sends a look to the hub: each change, and the wizard's first look. The document it answers with is cached.
pub fn save(ctx: &Ctx, look: Appearance) -> Task<Msg> {
    let body = serde_json::to_value(&look).unwrap_or(Value::Null);
    let c = ctx.client.clone();
    client::task(
        move || c.put::<Document>("/v1/appearance", &body),
        Msg::Saved,
    )
}

/// The first-run look: the built-in theme, chosen and applied now. The caller saves it to the hub.
pub fn choose_look(theme_id: &str) -> Appearance {
    let look = Appearance {
        theme: theme_id.to_string(),
        skin: None,
        chosen: true,
        ..theme::appearance()
    };
    let themes = read_cached().map(|d| d.themes).unwrap_or_default();
    theme::apply(&look, &themes);
    look
}

/// This device's own look, when it keeps one.
pub fn device_look() -> Option<Appearance> {
    read_device()
}

/// The look that names a saved theme for the hub: the hub's last confirmed choice with the theme set (and its own
/// skin), the rest kept. When this device keeps its own look, that look names the theme too.
pub fn choose_theme(theme_id: &str) -> Appearance {
    let name = |look: &mut Appearance| {
        look.theme = theme_id.to_string();
        look.skin = None;
        look.chosen = true;
    };
    if let Some(mut device) = read_device() {
        name(&mut device);
        remember_device(Some(&device));
    }
    let mut look = read_cached().map(|d| d.choice).unwrap_or_default();
    name(&mut look);
    look
}

/// Applies the look this device shows without a change in flight: its own look when it keeps one, else the hub's
/// confirmed choice, with the confirmed themes.
fn show_cached() {
    let device = read_device();
    let doc = read_cached();
    let themes = doc.as_ref().map(|d| d.themes.clone()).unwrap_or_default();
    let choice = device.or_else(|| doc.map(|d| d.choice)).unwrap_or_default();
    theme::apply(&choice, &themes);
}

/// Sets the light or dark mode chosen in the top bar. On a device with its own look the mode changes there. Otherwise
/// the hub's confirmed choice goes to the hub with only its mode changed, and `done` maps the hub's answer (see
/// [`mode_saved`]). The mode shows at once.
pub fn set_mode<M: Send + 'static>(
    ctx: &Ctx,
    mode: ModeChoice,
    done: fn(Result<Document, String>) -> M,
) -> Task<M> {
    if let Some(mut device) = read_device() {
        device.mode = mode;
        remember_device(Some(&device));
        show_cached();
        return Task::none();
    }
    let doc = read_cached();
    let themes = doc.as_ref().map(|d| d.themes.clone()).unwrap_or_default();
    let mut look = doc.map(|d| d.choice).unwrap_or_default();
    look.mode = mode;
    theme::apply(&look, &themes);
    let body = serde_json::to_value(&look).unwrap_or(Value::Null);
    let c = ctx.client.clone();
    client::task(move || c.put::<Document>("/v1/appearance", &body), done)
}

/// Takes the hub's answer to a top-bar mode change: a saved document is cached and applied. A failure puts the last
/// confirmed look back and returns the message to show.
pub fn mode_saved(res: Result<Document, String>) -> Result<(), String> {
    match res {
        Ok(doc) => {
            cache(&doc);
            show_cached();
            Ok(())
        }
        Err(e) => {
            show_cached();
            Err(format!("The look was not saved to the hub: {e}"))
        }
    }
}

/// The look on screen: this device's override, else the newest change the owner made, else the hub's confirmed choice.
fn current(st: &State) -> Appearance {
    st.device
        .clone()
        .or_else(|| st.queued.clone())
        .or_else(|| st.sent.clone())
        .or_else(|| st.doc.as_ref().map(|d| d.choice.clone()))
        .unwrap_or_default()
}

fn themes(st: &State) -> Vec<ThemeFile> {
    st.doc
        .as_ref()
        .map(|d| d.themes.clone())
        .unwrap_or_default()
}

/// Puts the look on screen now.
fn show(st: &State) {
    // The capture hook's look wins over the hub's answer as the console loads.
    let look = env_look().unwrap_or_else(|| current(st));
    theme::apply(&look, &themes(st));
}

/// A skin by its id, or the default skin when the id is unknown.
fn skin_of(id: &str) -> apz_theme::skin::Skin {
    apz_theme::skin::skin(id)
        .or_else(|| apz_theme::skin::skin(apz_theme::skin::DEFAULT_SKIN))
        .or_else(|| apz_theme::skin::skins().into_iter().next())
        .expect("apz-theme ships its built-in skins")
}

/// A theme by its id, saved or built in.
fn file_of(id: &str, saved: &[ThemeFile]) -> Option<ThemeFile> {
    saved
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .or_else(|| builtin::builtin(id))
}

/// Changes the look: it shows at once, then stays on this device or goes to the hub.
fn edit(st: &mut State, ctx: &Ctx, change: impl FnOnce(&mut Appearance)) -> Task<Msg> {
    let mut next = current(st);
    change(&mut next);
    if let Some(device) = st.device.as_mut() {
        *device = next;
        remember_device(st.device.as_ref());
        show(st);
        return Task::none();
    }
    st.queued = Some(next);
    show(st);
    send_next(st, ctx)
}

/// Sends the newest change when no save is in flight. One save goes at a time, so the hub answers in the order sent.
fn send_next(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.sent.is_some() {
        return Task::none();
    }
    let Some(look) = st.queued.take() else {
        return Task::none();
    };
    st.sent = Some(look.clone());
    save(ctx, look)
}

/// Turns this device's own look on (it keeps the look it shows now) or off (it drops that look and shows the hub's
/// choice again; nothing is sent to the hub).
fn device_only(st: &mut State, on: bool) {
    if on {
        st.device = Some(current(st));
        remember_device(st.device.as_ref());
        return;
    }
    st.device = None;
    remember_device(None);
    show(st);
}

pub fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Loaded(res) => {
            st.loading = false;
            if st.sent.is_some() {
                return Task::none();
            }
            st.device = read_device();
            match res {
                Ok(doc) => {
                    st.error = None;
                    if st.doc.as_ref() != Some(&doc) {
                        cache(&doc);
                        st.doc = Some(doc);
                        show(st);
                    }
                }
                Err(e) => st.error = Some(e),
            }
            Task::none()
        }
        Msg::Saved(res) => {
            st.sent = None;
            match res {
                Ok(doc) => {
                    st.error = None;
                    cache(&doc);
                    st.doc = Some(doc);
                }
                // The hub did not take the change: the confirmed look comes back, and the changes queued on it are dropped.
                Err(e) => {
                    st.queued = None;
                    st.error = Some(format!("The look was not saved to the hub: {e}"));
                }
            }
            show(st);
            send_next(st, ctx)
        }
        Msg::Event(event) => {
            if event.kind == "appearance" && !st.loading && st.sent.is_none() {
                load(st, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Skin(id) => edit(st, ctx, |look| look.skin = Some(id)),
        Msg::Theme(id) => edit(st, ctx, |look| look.theme = id),
        Msg::Background(id) => edit(st, ctx, |look| {
            look.background = id.map(BackgroundChoice::Preset)
        }),
        Msg::Mode(mode) => edit(st, ctx, |look| look.mode = mode),
        Msg::Density(density) => edit(st, ctx, |look| look.density = density),
        Msg::CodeSize(size) => edit(st, ctx, |look| {
            look.code_size = size.round().clamp(11.0, 16.0)
        }),
        Msg::DeviceOnly(on) => {
            device_only(st, on);
            Task::none()
        }
        Msg::AskDelete(id, name) => {
            st.confirm = Some((id, name));
            Task::none()
        }
        Msg::CancelDelete => {
            st.confirm = None;
            Task::none()
        }
        Msg::ConfirmDelete => {
            let Some((id, _)) = st.confirm.take() else {
                return Task::none();
            };
            let c = ctx.client.clone();
            client::task(
                move || {
                    let body = c.delete(&format!("/v1/appearance/themes/{id}"))?;
                    serde_json::from_value::<Document>(body).map_err(|e| e.to_string())
                },
                Msg::Deleted,
            )
        }
        Msg::Deleted(res) => {
            match res {
                Ok(doc) => {
                    st.error = None;
                    cache(&doc);
                    st.doc = Some(doc);
                    show(st);
                }
                Err(e) => st.error = Some(format!("The theme was not deleted: {e}")),
            }
            Task::none()
        }
        Msg::OpenEditor(_) | Msg::Duplicate(_) | Msg::Import | Msg::ImportVsCode => Task::none(),
    }
}

/// The hub's appearance events while the screen is shown.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

/// The web's grid of cards: `cols` equal columns, a short last row keeps the column widths, the cards in a row share
/// a height.
fn grid<'a>(cards: Vec<Element<'a, Msg>>, cols: usize) -> Element<'a, Msg> {
    let mut rows = column![].spacing(12).width(Length::Fill);
    let mut cards = cards.into_iter().peekable();
    while cards.peek().is_some() {
        let mut line = row![].spacing(12).width(Length::Fill);
        for _ in 0..cols {
            let cell: Element<'a, Msg> = match cards.next() {
                Some(card) => card,
                None => Space::new().into(),
            };
            line = line.push(container(cell).width(Length::FillPortion(1)));
        }
        rows = rows.push(line);
    }
    rows.into()
}

/// A labelled control in a column: the web's `.field`, its label in ink-2 at the control size.
fn field<'a>(label: &str, control: Element<'a, Msg>) -> Element<'a, Msg> {
    column![
        text(label.to_string())
            .font(SANS_MEDIUM)
            .size(TYPE_UI)
            .color(theme::palette().ink_2),
        control,
    ]
    .spacing(5)
    .into()
}

/// A card's surface: the web's `.card`, outlined in signal (two pixels) when it is the one in use.
fn surface(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |_: &Theme| {
        let p = theme::palette();
        container::Style {
            text_color: Some(p.ink),
            background: Some(p.raised.into()),
            border: Border {
                color: if selected { p.signal } else { p.rule },
                width: if selected { 2.0 } else { 1.0 },
                radius: theme::skin().radius_lg.into(),
            },
            shadow: theme::skin().shadow_card,
            snap: true,
        }
    }
}

/// A card that is a button (a skin, a background): the surface, its rule strong under the pointer.
fn look_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_: &Theme, status: button::Status| {
        let p = theme::palette();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(p.raised.into()),
            text_color: p.ink,
            border: Border {
                color: if selected {
                    p.signal
                } else if hovered {
                    p.rule_strong
                } else {
                    p.rule
                },
                width: if selected { 2.0 } else { 1.0 },
                radius: theme::skin().radius_lg.into(),
            },
            shadow: theme::skin().shadow_card,
            snap: true,
        }
    }
}

/// A skin's look: its default palette resolved in `mode`, drawn with the skin's own shape.
fn skin_look(s: &Skin, mode: Mode) -> Option<Resolved> {
    let file = builtin::builtin(&s.palette).or_else(|| builtin::builtins().into_iter().next())?;
    Some(apz_theme::resolve(&file, s, mode))
}

/// One skin's preview, as the web's `.look`: a mini sidebar and main column in the skin's colours, its label, its
/// primary button, then the skin's name and what it is about.
fn skin_card<'a>(s: &Skin, selected: bool, mode: Mode) -> Element<'a, Msg> {
    let Some(r) = skin_look(s, mode) else {
        return ui::faint("No palette").into();
    };
    let c = |name: &str| theme::color(r.tokens.get(name));
    let action = match s.action {
        ActionFrom::Ink => c("ink"),
        ActionFrom::Signal => c("signal"),
    };
    let (paper, sheet, rule, ink, on_action) =
        (c("paper"), c("sheet"), c("rule"), c("ink"), c("on-action"));
    let radius = s.radius;
    // One short bar of the mock's text, `portion` of the card's width and then `rest` empty.
    let line = move |portion: u16, rest: u16| -> Element<'a, Msg> {
        row![
            container(Space::new().width(Length::Fill).height(Length::Fixed(4.0)))
                .width(Length::FillPortion(portion))
                .style(move |_: &Theme| container::Style {
                    background: Some(Color { a: 0.3, ..ink }.into()),
                    border: Border::default().rounded(2.0),
                    ..Default::default()
                }),
            Space::new().width(Length::FillPortion(rest)),
        ]
        .into()
    };
    let mock_card = container(column![line(8, 2), line(5, 5)].spacing(4))
        .padding(6)
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(sheet.into()),
            border: Border {
                color: rule,
                width: 1.0,
                radius: radius.into(),
            },
            ..Default::default()
        });
    let run = container(text("Run").font(SANS_SEMIBOLD).size(9.0).color(on_action))
        .padding([2, 6])
        .style(move |_: &Theme| container::Style {
            background: Some(action.into()),
            border: Border::default().rounded(radius),
            ..Default::default()
        });
    let main = container(
        column![
            text("Agents").font(SANS_SEMIBOLD).size(9.0).color(ink),
            mock_card,
            row![run, Space::new().width(Length::Fill)].align_y(Alignment::Center),
        ]
        .spacing(6)
        .padding(8),
    )
    .width(Length::FillPortion(74))
    .height(Length::Fill);
    let side = container(Space::new())
        .width(Length::FillPortion(26))
        .height(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(sheet.into()),
            ..Default::default()
        });
    let divider = container(Space::new().width(Length::Fixed(1.0)).height(Length::Fill)).style(
        move |_: &Theme| container::Style {
            background: Some(rule.into()),
            ..Default::default()
        },
    );
    let mock = container(row![side, divider, main].height(Length::Fill))
        .height(Length::Fixed(96.0))
        .width(Length::Fill)
        .clip(true)
        .style(move |_: &Theme| container::Style {
            background: Some(paper.into()),
            border: Border {
                color: rule,
                width: 1.0,
                radius: radius.into(),
            },
            ..Default::default()
        });
    let p = theme::palette();
    let body = column![
        mock,
        text(s.name.clone())
            .font(SANS_SEMIBOLD)
            .size(TYPE_UI)
            .color(p.ink),
        text(s.about.clone())
            .font(SANS)
            .size(TYPE_META)
            .line_height(1.4)
            .color(p.ink_3),
    ]
    .spacing(8);
    button(body)
        .padding(10)
        .width(Length::Fill)
        .style(look_button(selected))
        .on_press(Msg::Skin(s.id.clone()))
        .into()
}

/// A dot in a palette card's row of five: the colour in a ring of the rule colour.
fn dot_swatch<'a>(color: Color) -> Element<'a, Msg> {
    container(
        Space::new()
            .width(Length::Fixed(14.0))
            .height(Length::Fixed(14.0)),
    )
    .style(move |_: &Theme| container::Style {
        background: Some(color.into()),
        border: Border {
            color: theme::palette().rule,
            width: 1.0,
            radius: 7.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Every built-in theme, then every saved one, as the web lists them.
fn all_themes(saved: &[ThemeFile]) -> Vec<ThemeFile> {
    let mut all = builtin::builtins();
    all.extend(saved.iter().cloned());
    all
}

/// One palette's card: its name, a built-in or yours, its words and five colours, then its actions. A saved theme is
/// edited or deleted; a built-in one is copied.
fn palette_card<'a>(file: &ThemeFile, selected: bool, mode: Mode) -> Element<'a, Msg> {
    let p = theme::palette();
    let r = apz_theme::resolve(file, &skin_of(&file.skin), mode);
    let dots: Vec<Element<'a, Msg>> = ["paper", "ink", "signal", "ok", "info"]
        .into_iter()
        .map(|name| dot_swatch(theme::color(r.tokens.get(name))))
        .collect();
    let saved = !builtin::is_builtin(&file.id);
    let tag = if saved {
        ui::badge("Yours", ui::Tone::Signal)
    } else {
        ui::badge("Built in", ui::Tone::Muted)
    };
    let top = row![
        text(file.name.clone())
            .font(SANS_SEMIBOLD)
            .size(TYPE_ITEM)
            .color(p.ink)
            .width(Length::Fill),
        tag,
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut pick_body = column![top].spacing(6);
    if !file.about.is_empty() {
        pick_body = pick_body.push(ui::faint(file.about.clone()));
    }
    pick_body = pick_body.push(row(dots).spacing(4));
    let pick = button(pick_body)
        .padding(0)
        .width(Length::Fill)
        .style(button::text)
        .on_press(Msg::Theme(file.id.clone()));
    let actions: Element<'a, Msg> = if saved {
        row![
            ui::btn_sm(
                "Edit",
                None,
                ui::Kind::Ghost,
                Some(Msg::OpenEditor(file.id.clone()))
            ),
            ui::btn_sm(
                "Delete",
                None,
                ui::Kind::Ghost,
                Some(Msg::AskDelete(file.id.clone(), file.name.clone())),
            ),
        ]
        .spacing(8)
        .into()
    } else {
        row![ui::btn_sm(
            "Duplicate and edit",
            None,
            ui::Kind::Ghost,
            Some(Msg::Duplicate(file.id.clone())),
        )]
        .into()
    };
    container(column![pick, actions].spacing(12))
        .padding(16)
        .width(Length::Fill)
        .style(surface(selected))
        .into()
}

/// The pattern of a background preset, drawn as the web draws its CSS gradients: dots, lines, diagonals or rings in
/// `color`, `scale` px apart.
struct PatternSwatch {
    pattern: Pattern,
    scale: f32,
    color: Color,
}

/// The drawing of a pattern swatch, kept between redraws, and the key of the inputs it was drawn from.
#[derive(Default)]
struct SwatchState {
    cache: canvas::Cache,
    key: Cell<u64>,
}

impl PatternSwatch {
    /// A hash of every input the drawing depends on. The size is not in it: the cache redraws when that changes.
    fn cache_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        std::mem::discriminant(&self.pattern).hash(&mut hasher);
        self.scale.to_bits().hash(&mut hasher);
        [self.color.r, self.color.g, self.color.b, self.color.a]
            .map(f32::to_bits)
            .hash(&mut hasher);
        hasher.finish()
    }
}

impl<M> canvas::Program<M> for PatternSwatch {
    type State = SwatchState;

    fn draw(
        &self,
        state: &SwatchState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // Nothing here depends on the cursor, so the whole swatch sits in the cache.
        let key = self.cache_key();
        if state.key.get() != key {
            state.cache.clear();
            state.key.set(key);
        }
        let geometry = state.cache.draw(renderer, bounds.size(), |frame| {
            let (w, h) = (bounds.width, bounds.height);
            let s = if self.scale > 0.0 { self.scale } else { 22.0 };
            let stroke = || Stroke::default().with_width(1.0).with_color(self.color);
            match self.pattern {
                Pattern::None => {}
                Pattern::Dots => {
                    let mut y = 0.0;
                    while y <= h {
                        let mut x = 0.0;
                        while x <= w {
                            frame.fill(&Path::circle(Point::new(x, y), 0.9), self.color);
                            x += s;
                        }
                        y += s;
                    }
                }
                Pattern::Grid | Pattern::Blueprint => {
                    let mut x = 0.0;
                    while x <= w {
                        frame.stroke(&Path::line(Point::new(x, 0.0), Point::new(x, h)), stroke());
                        x += s;
                    }
                    let mut y = 0.0;
                    while y <= h {
                        frame.stroke(&Path::line(Point::new(0.0, y), Point::new(w, y)), stroke());
                        y += s;
                    }
                }
                Pattern::Weave => {
                    // Lines running down to the right, 8 px apart across the stripe.
                    let step = 8.0 * std::f32::consts::SQRT_2;
                    let mut c = -h;
                    while c < w {
                        frame.stroke(
                            &Path::line(Point::new(c, 0.0), Point::new(c + h, h)),
                            stroke(),
                        );
                        c += step;
                    }
                }
                Pattern::Contours => {
                    let centre = Point::new(w * 0.3, h * 0.4);
                    let reach = w.max(h) * 1.5;
                    let mut r = s;
                    while r < reach {
                        frame.stroke(&Path::circle(centre, r), stroke());
                        r += s;
                    }
                }
            }
        });
        vec![geometry]
    }
}

/// One background preset's card: its pattern in the palette's pattern colour on the paper, then its name.
fn background_card<'a>(bg: &BgSpec, selected: bool, pattern: Color) -> Element<'a, Msg> {
    let p = theme::palette();
    let swatch = container(
        canvas::Canvas::new(PatternSwatch {
            pattern: bg.pattern,
            scale: bg.scale,
            color: pattern,
        })
        .width(Length::Fill)
        .height(Length::Fixed(64.0)),
    )
    .width(Length::Fill)
    .height(Length::Fixed(64.0))
    .clip(true)
    .style(move |_: &Theme| container::Style {
        background: Some(p.paper.into()),
        border: Border {
            color: p.rule,
            width: 1.0,
            radius: theme::skin().radius.into(),
        },
        ..Default::default()
    });
    let body = column![
        swatch,
        text(bg.name.clone()).font(SANS).size(TYPE_UI).color(p.ink),
    ]
    .spacing(6);
    button(body)
        .padding(8)
        .width(Length::Fill)
        .style(look_button(selected))
        .on_press(Msg::Background(Some(bg.id.clone())))
        .into()
}

/// The look a choice puts in force, as the web resolves it: the theme it names (the default when none is saved or
/// built in), the skin (the choice's own, else the theme's), the background's id, and the tokens in `mode`.
struct Force {
    theme_id: String,
    skin: String,
    background: String,
    res: Resolved,
}

fn force(a: &Appearance, saved: &[ThemeFile], mode: Mode) -> Force {
    let file = file_of(&a.theme, saved)
        .or_else(|| builtin::builtin(builtin::DEFAULT_THEME))
        .or_else(|| builtin::builtins().into_iter().next())
        .expect("apz-theme ships its built-in themes");
    let skin_id = a.skin.clone().unwrap_or_else(|| file.skin.clone());
    let skin = skin_of(&skin_id);
    let res = apz_theme::resolve(&file, &skin, mode);
    let background = match &a.background {
        Some(BackgroundChoice::Preset(id)) => id.clone(),
        Some(BackgroundChoice::Spec(spec)) => spec.id.clone(),
        None => res.background.id.clone(),
    };
    Force {
        theme_id: file.id.clone(),
        skin: skin.id.clone(),
        background,
        res,
    }
}

/// The Display card: the mode and the density as segments, the code size as a stepper.
fn display<'a>(a: &Appearance) -> Element<'a, Msg> {
    let p = theme::palette();
    let size = a.code_size;
    let code_size = row![
        ui::btn_sm(
            "−",
            None,
            ui::Kind::Ghost,
            (size > CODE_MIN).then_some(Msg::CodeSize(size - 1.0)),
        ),
        ui::mono(format!("{size:.0}px"), TYPE_META, p.ink),
        ui::btn_sm(
            "+",
            None,
            ui::Kind::Ghost,
            (size < CODE_MAX).then_some(Msg::CodeSize(size + 1.0)),
        ),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    column![
        field(
            "Mode",
            ui::seg(
                &[
                    (ModeChoice::System, "System"),
                    (ModeChoice::Light, "Light"),
                    (ModeChoice::Dark, "Dark"),
                ],
                a.mode,
                Msg::Mode,
            ),
        ),
        field(
            "Density",
            ui::seg(
                &[
                    (Density::Compact, "Compact"),
                    (Density::Default, "Default"),
                    (Density::Comfortable, "Comfortable"),
                ],
                a.density,
                Msg::Density,
            ),
        ),
        field("Code size", code_size.into()),
    ]
    .spacing(12)
    .into()
}

/// "This device only" as the web's switch, with the note on what it changes.
fn device_switch<'a>(on: bool) -> Element<'a, Msg> {
    let p = theme::palette();
    let knob = container(
        Space::new()
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0)),
    )
    .style(|_: &Theme| container::Style {
        background: Some(Color::WHITE.into()),
        border: Border::default().rounded(8.0),
        shadow: Shadow {
            color: Color {
                a: 0.25,
                ..Color::BLACK
            },
            offset: Vector::new(0.0, 1.0),
            blur_radius: 2.0,
        },
        ..Default::default()
    });
    let inner = if on {
        row![Space::new().width(Length::Fill), knob]
    } else {
        row![knob, Space::new().width(Length::Fill)]
    }
    .align_y(Alignment::Center);
    let track = button(inner)
        .padding(2)
        .width(Length::Fixed(34.0))
        .height(Length::Fixed(20.0))
        .style(move |_: &Theme, _: button::Status| button::Style {
            background: Some(if on { p.signal } else { p.rule_strong }.into()),
            border: Border::default().rounded(10.0),
            ..Default::default()
        })
        .on_press(Msg::DeviceOnly(!on));
    let note = if on {
        "In force on this device: changes here stay on this device and the hub keeps its own look. Turning this off clears this device’s look and follows the hub’s choice again; nothing from this device is sent to the hub."
    } else {
        "In force: the hub’s look, the same on every device. Changes are stored on the hub."
    };
    column![
        row![
            track,
            text("This device only")
                .font(SANS)
                .size(TYPE_UI)
                .color(p.ink),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        ui::faint(note),
    ]
    .spacing(10)
    .into()
}

/// The dialog that confirms a delete, as the web's confirm: it names the theme and what falls back.
fn delete_dialog<'a>(name: &str) -> Element<'a, Msg> {
    let body = ui::prose(format!(
        "Delete the theme \"{name}\" from the hub? Anything that uses it falls back to the default look."
    ));
    ui::modal(
        "Delete the theme?",
        body.into(),
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelDelete)),
            ui::btn("Delete", None, ui::Kind::Danger, Some(Msg::ConfirmDelete)),
        ],
        Msg::CancelDelete,
        420.0,
    )
}

pub fn view(st: &State) -> Element<'_, Msg> {
    let a = current(st);
    let saved = themes(st);
    let mode = if theme::is_light() {
        Mode::Light
    } else {
        Mode::Dark
    };
    let f = force(&a, &saved, mode);
    let head = ui::view_head_with_intro(
        "Appearance",
        "One look for this hub, on the web and on the desktop.",
        Vec::new(),
    );
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if st.loading && st.doc.is_none() {
        parts.push(ui::loading());
    }
    if let Some(e) = st.error.as_ref() {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = st.notice.as_ref() {
        parts.push(ui::faint(n.clone()).into());
    }
    let skins: Vec<Element<'_, Msg>> = apz_theme::skin::skins()
        .iter()
        .map(|s| skin_card(s, s.id == f.skin, mode))
        .collect();
    parts.push(ui::card("Skin", grid(skins, 3)));
    let palettes: Vec<Element<'_, Msg>> = all_themes(&saved)
        .iter()
        .map(|file| palette_card(file, file.id == f.theme_id, mode))
        .collect();
    parts.push(ui::card_with(
        "Palette",
        vec![
            ui::btn_sm(
                "Import theme…",
                None,
                ui::Kind::Secondary,
                Some(Msg::Import),
            ),
            ui::btn_sm(
                "Import from VS Code…",
                None,
                ui::Kind::Secondary,
                Some(Msg::ImportVsCode),
            ),
        ],
        grid(palettes, 3),
    ));
    let pattern = theme::color(f.res.tokens.get("pattern"));
    let backgrounds: Vec<Element<'_, Msg>> = background::presets()
        .iter()
        .map(|bg| background_card(bg, bg.id == f.background, pattern))
        .collect();
    parts.push(ui::card("Background", grid(backgrounds, 4)));
    parts.push(ui::card("Display", display(&a)));
    parts.push(ui::card(
        "Where changes go",
        device_switch(st.device.is_some()),
    ));
    let body: Element<'_, Msg> = column(parts).spacing(20).width(Length::Fill).into();
    let page = ui::scroll(ui::sheet(head, body)).width(Length::Fill).into();
    ui::with_modal(
        page,
        st.confirm.as_ref().map(|(_, name)| delete_dialog(name)),
    )
}
