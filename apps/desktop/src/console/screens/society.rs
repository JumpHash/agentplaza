//! The Society screen (parity with the web console's `society` view): the agents as a graph of
//! who knows whom, drawn on a canvas, and the selected agent's opinions in both directions.
//!
//! Above `GROUP_AT` agents the graph opens grouped by family: a family is a cluster node, a click
//! pops it open into its roles (a role with one agent shows that agent), and a role pops open into
//! its agents. The breadcrumb climbs back up, and the toggle switches between the family level and
//! the agent level at any size.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use iced::widget::{Space, canvas, column, container, responsive, row, text};
use iced::{
    Alignment, Background, Color, Element, Length, Point, Rectangle, Renderer, Size, Task, Theme,
    border, mouse,
};
use serde::Deserialize;
use serde_json::Value;

use super::{Id, sanctions};
use crate::console::ui::{self, Kind, Tone};
use crate::console::{Ctx, client};

/// The layout's world box. The first layout spreads the agents over all of it, and the view maps
/// the box onto the graph area, so the graph grows and shrinks with the panel.
const LW: f32 = 760.0;
const LH: f32 = 700.0;
/// An agent's disc radius, as drawn (the web's DISC).
const DISC: f32 = 20.0;
/// How far an agent's name tag reaches below its centre (the web's TAG_DOWN).
const TAG_DOWN: f32 = 40.0;
/// Clear space kept between two nodes' footprints.
const GAP: f32 = 10.0;
/// Collision passes. A box too crowded to clear in this many is left with the overlap it has.
const PASSES: usize = 400;
const FIRST_STEPS: usize = 300;
const LATER_STEPS: usize = 150;
const REST: f32 = 150.0;
const DISAGREE: f32 = 0.25;
const NOTE_PREVIEW: usize = 220;
/// Above this many agents the graph opens at the family level.
const GROUP_AT: usize = 24;
/// Name tags and group labels are sans text of this size (the web's 11.5px), with the web's width
/// estimate per character and this padding.
const LABEL_SIZE: f32 = 11.5;
const LABEL_CHAR: f32 = 7.4;
const LABEL_PAD: f32 = 8.0;
/// The pop: how long the nodes that come into view take to grow out of the ones that went, and the
/// step between frames.
const POP: Duration = Duration::from_millis(250);
const FRAME: Duration = Duration::from_millis(16);
fn red() -> Color {
    crate::theme::palette().crit
}
fn amber() -> Color {
    crate::theme::palette().warn
}
fn green() -> Color {
    crate::theme::palette().ok
}

/// The society as `GET /v1/society` returns it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Society {
    #[serde(default)]
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub bonds: Vec<Bond>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    pub cid: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
    /// The server's tier for the role, when it sends one; "orchestrator" is the command family.
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub reputation: f32,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub wallet: f64,
}

/// A directed contact edge: the holder `from` and its opinion of `to`.
#[derive(Debug, Clone, Deserialize)]
pub struct Bond {
    pub from: u64,
    pub to: u64,
    pub kind: String,
    #[serde(default)]
    pub strength: f32,
    #[serde(default)]
    pub note: Option<String>,
    pub opinion: Opinion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Opinion {
    /// Expected trust, in [0, 1].
    #[serde(default)]
    pub e: f32,
    /// Uncertainty, in [0, 1].
    #[serde(default)]
    pub u: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

/// What a node stands for: one agent, or a family or role cluster of agents. A role names its
/// family too, because one role name can sit in more than one family.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    Agent(u64),
    Family(&'static str),
    Role(&'static str, String),
}

pub use apz_engine::roles::role_family;
use apz_engine::roles::role_label;

/// A family's name on screen.
pub fn family_label(family: &str) -> &'static str {
    match family {
        "command" => "Command",
        "build" => "Build",
        "operations" => "Operations",
        "knowledge" => "Knowledge",
        "money" => "Money",
        _ => "Other",
    }
}

fn family_of(a: &Agent) -> &'static str {
    role_family(&a.role, &a.tier)
}

/// Whether an agent is in a node's group: the agent itself, its family, or its role.
fn in_group(a: &Agent, key: &Key) -> bool {
    match key {
        Key::Agent(cid) => a.cid == *cid,
        Key::Family(f) => family_of(a) == *f,
        Key::Role(f, r) => family_of(a) == *f && a.role == *r,
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    data: Option<Society>,
    loaded: bool,
    /// Where each node sits, by key; kept across refreshes so the graph does not jump.
    pos: HashMap<Key, Pt>,
    selected: Option<Key>,
    /// The peers whose private notes are expanded in the panel.
    open_notes: HashSet<u64>,
    /// The agents with an active suspension: they carry the suspended mark.
    suspended: HashSet<u64>,
    /// The family toggle; None follows the agent count.
    group: Option<bool>,
    /// The families and roles open, outermost first. Only used while grouped.
    nav: Vec<Key>,
    scene: Scene,
    /// The pop in progress.
    anim: Option<Anim>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Society, String>),
    /// Select an agent, or clear the selection with `None`.
    Pick(Option<Key>),
    /// Pop a family or a role open one level, and select it.
    Open(Key),
    /// Select an agent, and open the family and role it sits in.
    Member(u64),
    /// Go up to the first `n` levels of the path (the breadcrumb, Back).
    Level(usize),
    /// Esc: up one level, when a level is open.
    Up,
    /// Switch between the family level and the agent level.
    Group,
    /// One frame of the pop.
    Frame,
    /// The owner rates an agent: +1 trust more, -1 trust less.
    Rate(u64, f32),
    Rated(Result<Value, String>),
    /// Expand or collapse a peer's private note.
    Note(u64),
    /// The sanctions register, read once per refresh for the suspended marks.
    Sanctions(Result<sanctions::Register, String>),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the society (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let sanctions_client = ctx.client.clone();
    Task::batch([
        client::task(move || client.get::<Society>("/v1/society"), Msg::Loaded),
        client::task(move || sanctions::active(&sanctions_client), Msg::Sanctions),
    ])
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(data)) => {
            state.error = None;
            state.loaded = true;
            if state
                .selected
                .as_ref()
                .is_some_and(|key| !data.agents.iter().any(|a| in_group(a, key)))
            {
                state.selected = None;
            }
            state.data = Some(data);
            refresh(state)
        }
        Msg::Loaded(Err(e)) => {
            // Keep the last good view; the next poll tries again.
            state.loaded = true;
            state.error = Some(e);
            Task::none()
        }
        Msg::Pick(key) => {
            state.selected = key;
            state.open_notes.clear();
            Task::none()
        }
        Msg::Open(key) => {
            state.nav = match &key {
                Key::Family(f) => vec![Key::Family(f)],
                Key::Role(f, r) => vec![Key::Family(f), Key::Role(f, r.clone())],
                Key::Agent(_) => state.nav.clone(),
            };
            state.selected = Some(key);
            state.open_notes.clear();
            refresh(state)
        }
        Msg::Member(cid) => {
            let path = state
                .data
                .as_ref()
                .and_then(|d| d.agents.iter().find(|a| a.cid == cid))
                .map(|a| {
                    vec![
                        Key::Family(family_of(a)),
                        Key::Role(family_of(a), a.role.clone()),
                    ]
                });
            if let (Some(path), true) = (path, is_grouped(state)) {
                state.nav = path;
            }
            state.selected = Some(Key::Agent(cid));
            state.open_notes.clear();
            refresh(state)
        }
        Msg::Level(n) => {
            state.nav.truncate(n);
            refresh(state)
        }
        Msg::Up => {
            if state.nav.pop().is_some() {
                return refresh(state);
            }
            // At the top level Esc clears the selection, as the web's does.
            if state.selected.take().is_some() {
                state.open_notes.clear();
            }
            Task::none()
        }
        Msg::Group => {
            let on = is_grouped(state);
            state.group = Some(!on);
            state.nav.clear();
            refresh(state)
        }
        Msg::Frame => match &state.anim {
            Some(a) if a.at.elapsed() < POP => tick(),
            _ => {
                state.anim = None;
                Task::none()
            }
        },
        Msg::Note(peer) => {
            if !state.open_notes.remove(&peer) {
                state.open_notes.insert(peer);
            }
            Task::none()
        }
        Msg::Rate(cid, value) => {
            let client = ctx.client.clone();
            let body = serde_json::json!({ "value": value });
            client::task(
                move || client.post::<Value>(&format!("/v1/agents/{cid}/rating"), &body),
                Msg::Rated,
            )
        }
        Msg::Rated(result) => {
            if let Err(e) = result {
                state.error = Some(e);
            }
            load(state, ctx)
        }
        Msg::Sanctions(result) => {
            // Keep the last marks when the register cannot be read; the next refresh tries again.
            if let Ok(register) = result {
                state.suspended = register
                    .suspensions
                    .iter()
                    .filter(|s| s.active)
                    .map(|s| s.cid)
                    .collect();
            }
            Task::none()
        }
    }
}

/// Whether the graph is grouped by family now.
fn is_grouped(state: &State) -> bool {
    state
        .data
        .as_ref()
        .is_some_and(|d| state.group.unwrap_or(d.agents.len() > GROUP_AT))
}

/// Rebuilds the scene for the current level and lays it out. The nodes that come into view grow
/// out of the spot the ones that went out of view left.
fn refresh(state: &mut State) -> Task<Msg> {
    let grouped = is_grouped(state);
    if !grouped {
        state.nav.clear();
    }
    let Some(data) = state.data.as_ref() else {
        return Task::none();
    };
    let live: HashSet<u64> = data.agents.iter().map(|a| a.cid).collect();
    state.pos.retain(|key, _| match key {
        Key::Agent(cid) => live.contains(cid),
        _ => true,
    });
    let scene = build_scene(data, grouped, &state.nav);
    let old: HashSet<Key> = state.scene.nodes.iter().map(|n| n.key.clone()).collect();
    let next: HashSet<Key> = scene.nodes.iter().map(|n| n.key.clone()).collect();
    let gone: Vec<Pt> = old
        .difference(&next)
        .filter_map(|k| state.pos.get(k).copied())
        .collect();
    let fresh: HashSet<Key> = next.difference(&old).cloned().collect();
    layout(&scene.nodes, &scene.links, &mut state.pos);
    state.scene = scene;
    if old.is_empty() || fresh.is_empty() {
        return Task::none();
    }
    let origin = if gone.is_empty() {
        Pt {
            x: LW / 2.0,
            y: LH / 2.0,
        }
    } else {
        let n = gone.len() as f32;
        Pt {
            x: gone.iter().map(|p| p.x).sum::<f32>() / n,
            y: gone.iter().map(|p| p.y).sum::<f32>() / n,
        }
    };
    state.anim = Some(Anim {
        at: Instant::now(),
        origin,
        keys: fresh,
    });
    tick()
}

/// The next frame of the pop. The screen has no frame clock of its own, so a short timer drives it.
fn tick() -> Task<Msg> {
    Task::perform(async { std::thread::sleep(FRAME) }, |_| Msg::Frame)
}

/// The pop in progress: the nodes in `keys` grow out of `origin`.
#[derive(Debug)]
struct Anim {
    at: Instant,
    origin: Pt,
    keys: HashSet<Key>,
}

/// Ease-out over the pop's duration, from 0 to 1.
fn ease(elapsed: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / POP.as_secs_f32()).min(1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &state.data {
        _ if !state.loaded => ui::loading(),
        None => ui::error_line(
            state
                .error
                .as_deref()
                .unwrap_or("Could not load the society."),
        ),
        Some(d) if d.agents.is_empty() => ui::empty(
            "society",
            "No agents yet",
            "Spawn agents and they show up here, with what they think of one another.",
        ),
        Some(d) => {
            let grouped = is_grouped(state);
            let grow = state.anim.as_ref().map(|a| Grow {
                origin: a.origin,
                t: ease(a.at.elapsed()),
                keys: &a.keys,
            });
            let (scene, pos, suspended) = (&state.scene, &state.pos, &state.suspended);
            let selected = state.selected.as_ref();
            // The graph is as tall as the web's SVG: the world box (LW by LH) at the width it is given.
            let graph = responsive(move |size: Size| -> Element<'a, Msg> {
                canvas(Graph {
                    scene,
                    pos,
                    selected,
                    grow,
                    suspended,
                })
                .width(Length::Fill)
                .height(Length::Fixed(size.width * LH / LW))
                .into()
            })
            .width(Length::Fill)
            .height(Length::Shrink);
            let panel = match &state.selected {
                Some(Key::Agent(cid)) => match d.agents.iter().find(|a| a.cid == *cid) {
                    Some(me) => detail(state, d, me),
                    None => legend(),
                },
                Some(key) if d.agents.iter().any(|a| in_group(a, key)) => group_panel(d, key),
                _ => legend(),
            };
            // The graph card, then the panel card at a fixed 340, as the web's `.soc-wrap`: both as tall as
            // their content, top aligned, and the page scrolls.
            let mut left = column![toolbar(grouped, state.nav.len())]
                .spacing(12)
                .width(Length::Fill);
            if grouped {
                left = left.push(crumbs(&state.nav));
            }
            left = left.push(graph);
            row![
                container(left)
                    .padding(16)
                    .width(Length::Fill)
                    .style(surface),
                container(column![panel].width(Length::Fill))
                    .padding(16)
                    .width(Length::Fixed(340.0))
                    .style(surface),
            ]
            .spacing(12)
            .into()
        }
    };
    ui::sheet(ui::view_head("Society", vec![]), body)
}

// ---- Scene -----------------------------------------------------------------

/// What the graph draws at the current level: the nodes in view and the links among them.
#[derive(Debug, Default)]
struct Scene {
    nodes: Vec<Node>,
    links: Vec<Link>,
}

#[derive(Debug)]
struct Node {
    key: Key,
    /// The name tag of an agent, or the label inside a cluster.
    label: String,
    /// The agents it stands for, by cid, sorted.
    members: Vec<u64>,
    /// The box in world units: the robot's side for an agent, sized by member count for a cluster.
    w: f32,
    h: f32,
    fp: Foot,
}

#[derive(Debug)]
struct Link {
    from: Key,
    to: Key,
    /// The summed contact strength.
    strength: f32,
    /// The expected trust: the holder's opinion for a directed link, the mean over the bonds for an
    /// aggregated one.
    e: f32,
    /// A bond drawn with its direction (agent level); aggregated links are undirected.
    directed: bool,
    mutual: bool,
}

/// The nodes in view at this level, and the links among them.
fn build_scene(d: &Society, grouped: bool, nav: &[Key]) -> Scene {
    let mut count: HashMap<Key, usize> = HashMap::new();
    for a in &d.agents {
        *count
            .entry(Key::Role(family_of(a), a.role.clone()))
            .or_insert(0) += 1;
    }
    let key_of: HashMap<u64, Key> = d
        .agents
        .iter()
        .map(|a| {
            (
                a.cid,
                if grouped {
                    visible(a, nav, &count)
                } else {
                    Key::Agent(a.cid)
                },
            )
        })
        .collect();
    let mut order: Vec<Key> = Vec::new();
    let mut groups: HashMap<Key, Vec<&Agent>> = HashMap::new();
    for a in &d.agents {
        let key = key_of[&a.cid].clone();
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(a);
    }
    let nodes: Vec<Node> = order.iter().map(|k| node_of(k, &groups[k])).collect();
    let links = links(&d.bonds, |cid| key_of.get(&cid).cloned(), grouped);
    Scene { nodes, links }
}

/// The node an agent is drawn in at this level: its family, or once the family is open, its role
/// (or its own node when the role has one agent, or the role is open).
fn visible(a: &Agent, nav: &[Key], count: &HashMap<Key, usize>) -> Key {
    let fam = family_of(a);
    match nav.first() {
        Some(Key::Family(open)) if *open == fam => {
            let role_open =
                matches!(nav.get(1), Some(Key::Role(f, r)) if *f == fam && *r == a.role);
            let many = count
                .get(&Key::Role(fam, a.role.clone()))
                .is_some_and(|n| *n >= 2);
            if many && !role_open {
                Key::Role(fam, a.role.clone())
            } else {
                Key::Agent(a.cid)
            }
        }
        _ => Key::Family(fam),
    }
}

/// A node for a key and the agents it stands for.
fn node_of(key: &Key, members: &[&Agent]) -> Node {
    if let (Key::Agent(_), [a]) = (key, members) {
        return Node {
            key: key.clone(),
            label: label_of(a),
            members: vec![a.cid],
            w: 2.0 * DISC,
            h: 2.0 * DISC,
            fp: foot(a),
        };
    }
    let mut cids: Vec<u64> = members.iter().map(|a| a.cid).collect();
    cids.sort_unstable();
    let label = match key {
        Key::Family(f) => family_label(f).to_string(),
        Key::Role(_, r) if r.trim().is_empty() => "unassigned".to_string(),
        Key::Role(_, r) => clip(r),
        Key::Agent(_) => String::new(),
    };
    // A box sized by its member count on a square root scale; its label sits under it.
    let side = box_side(cids.len());
    let tag_half = tag_width(&label) / 2.0;
    Node {
        key: key.clone(),
        label,
        members: cids,
        w: side,
        h: side,
        fp: Foot {
            half: (side / 2.0).max(tag_half),
            up: side / 2.0,
            down: side / 2.0 + 20.0,
        },
    }
}

/// A group box's side in world units: the web's 58 + 24 * sqrt(count).
fn box_side(count: usize) -> f32 {
    58.0 + 24.0 * (count as f32).sqrt()
}

/// The links among the nodes in view. At the agent level each bond is kept with its direction. Once
/// grouped, the bonds between two nodes become one undirected link: strengths summed, trust averaged.
/// Bonds inside one node are left out.
fn links(bonds: &[Bond], key_of: impl Fn(u64) -> Option<Key>, grouped: bool) -> Vec<Link> {
    if !grouped {
        let pairs: HashSet<(u64, u64)> = bonds.iter().map(|b| (b.from, b.to)).collect();
        return bonds
            .iter()
            .filter_map(|b| {
                let (from, to) = (key_of(b.from)?, key_of(b.to)?);
                (from != to).then(|| Link {
                    from,
                    to,
                    strength: b.strength,
                    e: b.opinion.e,
                    directed: true,
                    mutual: pairs.contains(&(b.to, b.from)),
                })
            })
            .collect();
    }
    let mut agg: BTreeMap<(Key, Key), (f32, f32, usize)> = BTreeMap::new();
    for b in bonds {
        let (Some(a), Some(z)) = (key_of(b.from), key_of(b.to)) else {
            continue;
        };
        if a == z {
            continue;
        }
        let pair = if a <= z { (a, z) } else { (z, a) };
        let entry = agg.entry(pair).or_insert((0.0, 0.0, 0));
        entry.0 += b.strength;
        entry.1 += b.opinion.e;
        entry.2 += 1;
    }
    agg.into_iter()
        .map(|((from, to), (strength, esum, n))| Link {
            from,
            to,
            strength,
            e: esum / n as f32,
            directed: false,
            mutual: false,
        })
        .collect()
}

// ---- Layout ----------------------------------------------------------------

/// What a node takes up around its centre, in world units.
#[derive(Debug, Clone, Copy)]
struct Foot {
    /// Half the width: the robot, or its name tag when that is wider.
    half: f32,
    /// The room above the centre: the antenna and its tip.
    up: f32,
    /// The room below the centre: the name tag.
    down: f32,
}

fn foot(a: &Agent) -> Foot {
    Foot {
        half: DISC.max(tag_width(&label_of(a)) / 2.0),
        up: DISC,
        down: TAG_DOWN,
    }
}

/// How far two footprints overlap on each axis, with their centres at `p` and `q`. They clash
/// only when both are positive.
fn overlap(p: Pt, fp: Foot, q: Pt, fq: Foot) -> (f32, f32) {
    let (dx, dy) = (q.x - p.x, q.y - p.y);
    // Stacked vertically, the lower node's name tag must clear the upper node's antenna.
    let stack = if dy >= 0.0 {
        fp.down + fq.up
    } else {
        fq.down + fp.up
    };
    (fp.half + fq.half + GAP - dx.abs(), stack + GAP - dy.abs())
}

struct Body {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    fixed: bool,
    fp: Foot,
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

/// Places the nodes. Nodes already placed stay where they are; only new ones move, under the
/// springs and repulsion of the whole graph, and then clear of everyone's footprint.
fn layout(nodes: &[Node], links: &[Link], pos: &mut HashMap<Key, Pt>) {
    let fresh: Vec<usize> = (0..nodes.len())
        .filter(|&i| !pos.contains_key(&nodes[i].key))
        .collect();
    if fresh.is_empty() {
        return;
    }
    let placed = nodes.iter().any(|n| pos.contains_key(&n.key));
    if placed {
        // A newcomer takes the clearest spot near its contacts; the nodes already placed do not move.
        for &i in &fresh {
            let spot = clearest(nodes, i, links, pos);
            pos.insert(nodes[i].key.clone(), spot);
        }
    } else {
        // The first layout rings the nodes round the box, as the prototype does.
        let n = fresh.len().max(1) as f32;
        let ring = if fresh.len() <= 1 {
            0.0
        } else {
            (120.0 + 22.0 * n).min(280.0)
        };
        for (k, &i) in fresh.iter().enumerate() {
            let angle = -std::f32::consts::FRAC_PI_2 + k as f32 * 2.0 * std::f32::consts::PI / n;
            pos.insert(
                nodes[i].key.clone(),
                Pt {
                    x: LW / 2.0 + ring * angle.cos(),
                    y: LH / 2.0 + ring * angle.sin() * 0.85,
                },
            );
        }
    }

    let is_fresh: HashSet<usize> = fresh.iter().copied().collect();
    let mut bodies: Vec<Body> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let p = pos.get(&n.key).copied().unwrap_or(Pt {
                x: LW / 2.0,
                y: LH / 2.0,
            });
            Body {
                x: p.x,
                y: p.y,
                vx: 0.0,
                vy: 0.0,
                fixed: !is_fresh.contains(&i),
                fp: n.fp,
            }
        })
        .collect();
    let index: HashMap<&Key, usize> = nodes.iter().enumerate().map(|(i, n)| (&n.key, i)).collect();

    // One spring per unordered pair; stronger contacts pull harder.
    let mut springs: Vec<(usize, usize, f32)> = Vec::new();
    for link in links {
        let (Some(&a), Some(&z)) = (index.get(&link.from), index.get(&link.to)) else {
            continue;
        };
        if a == z {
            continue;
        }
        let (lo, hi) = (a.min(z), a.max(z));
        let k = 0.01 + 0.02 * clamp(link.strength / 10.0, 0.0, 1.0);
        match springs.iter_mut().find(|s| s.0 == lo && s.1 == hi) {
            Some(s) => s.2 = k.max(s.2),
            None => springs.push((lo, hi, k)),
        }
    }

    // A first layout is the ring as it is; later ones settle under the springs and repulsion.
    let steps = if !placed {
        0
    } else if fresh.len() == nodes.len() {
        FIRST_STEPS
    } else {
        LATER_STEPS
    };
    for _ in 0..steps {
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                let (dx, dy) = (bodies[j].x - bodies[i].x, bodies[j].y - bodies[i].y);
                let d = (dx * dx + dy * dy).sqrt().max(1.0);
                let f = 16000.0 / (d * d);
                let (fx, fy) = (f * dx / d, f * dy / d);
                bodies[i].vx -= fx;
                bodies[i].vy -= fy;
                bodies[j].vx += fx;
                bodies[j].vy += fy;
            }
        }
        for &(a, z, k) in &springs {
            let (dx, dy) = (bodies[z].x - bodies[a].x, bodies[z].y - bodies[a].y);
            let d = (dx * dx + dy * dy).sqrt().max(1.0);
            let f = k * (d - REST);
            let (fx, fy) = (f * dx / d, f * dy / d);
            bodies[a].vx += fx;
            bodies[a].vy += fy;
            bodies[z].vx -= fx;
            bodies[z].vy -= fy;
        }
        for b in &mut bodies {
            // Centring, then damping; placed nodes keep their coordinates.
            b.vx = clamp((b.vx + (LW / 2.0 - b.x) * 0.004) * 0.8, -25.0, 25.0);
            b.vy = clamp((b.vy + (LH / 2.0 - b.y) * 0.004) * 0.8, -25.0, 25.0);
            if b.fixed {
                continue;
            }
            b.x = clamp(b.x + b.vx, b.fp.half, LW - b.fp.half);
            b.y = clamp(b.y + b.vy, b.fp.up, LH - b.fp.down);
        }
    }
    separate(&mut bodies);
    for (n, b) in nodes.iter().zip(&bodies) {
        pos.insert(n.key.clone(), Pt { x: b.x, y: b.y });
    }
}

/// The clearest spot for node `i`: the free room to its nearest neighbour, capped so that any spot
/// with room is as good as another, then the spot nearest its contacts among those.
fn clearest(nodes: &[Node], i: usize, links: &[Link], pos: &HashMap<Key, Pt>) -> Pt {
    let me = &nodes[i];
    let fp = me.fp;
    let others: Vec<(Pt, Foot)> = nodes
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != i)
        .filter_map(|(_, o)| pos.get(&o.key).map(|p| (*p, o.fp)))
        .collect();
    let peers: Vec<Pt> = links
        .iter()
        .filter_map(|l| {
            let other = if l.from == me.key {
                &l.to
            } else if l.to == me.key {
                &l.from
            } else {
                return None;
            };
            pos.get(other).copied()
        })
        .collect();
    let target = if peers.is_empty() {
        Pt {
            x: LW / 2.0,
            y: LH / 2.0,
        }
    } else {
        let n = peers.len() as f32;
        Pt {
            x: peers.iter().map(|p| p.x).sum::<f32>() / n,
            y: peers.iter().map(|p| p.y).sum::<f32>() / n,
        }
    };
    let mut best = target;
    let mut best_score = f32::MIN;
    let mut x = fp.half;
    while x <= LW - fp.half {
        let mut y = fp.up;
        while y <= LH - fp.down {
            let c = Pt { x, y };
            let room = others.iter().fold(40.0_f32, |room, (q, fq)| {
                let (ox, oy) = overlap(c, fp, *q, *fq);
                room.min((-ox).max(-oy))
            });
            let score = room - 0.02 * ((x - target.x).powi(2) + (y - target.y).powi(2)).sqrt();
            if score > best_score {
                best_score = score;
                best = c;
            }
            y += 6.0;
        }
        x += 6.0;
    }
    best
}

/// Pushes the footprints apart until none clash, or the passes run out. A clash moves both nodes
/// along the axis where they overlap least; a placed node stays put. Nodes are kept in the box.
fn separate(bodies: &mut [Body]) {
    for _ in 0..PASSES {
        let mut clashed = false;
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                let p = Pt {
                    x: bodies[i].x,
                    y: bodies[i].y,
                };
                let q = Pt {
                    x: bodies[j].x,
                    y: bodies[j].y,
                };
                let (ox, oy) = overlap(p, bodies[i].fp, q, bodies[j].fp);
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }
                let (wi, wj) = match (bodies[i].fixed, bodies[j].fixed) {
                    (true, true) => continue,
                    (true, false) => (0.0, 1.0),
                    (false, true) => (1.0, 0.0),
                    (false, false) => (0.5, 0.5),
                };
                clashed = true;
                if ox < oy {
                    let s = if q.x < p.x { -1.0 } else { 1.0 };
                    bodies[j].x += s * (ox + 0.5) * wj;
                    bodies[i].x -= s * (ox + 0.5) * wi;
                } else {
                    let s = if q.y < p.y { -1.0 } else { 1.0 };
                    bodies[j].y += s * (oy + 0.5) * wj;
                    bodies[i].y -= s * (oy + 0.5) * wi;
                }
            }
        }
        for b in bodies.iter_mut().filter(|b| !b.fixed) {
            b.x = clamp(b.x, b.fp.half, LW - b.fp.half);
            b.y = clamp(b.y, b.fp.up, LH - b.fp.down);
        }
        if !clashed {
            break;
        }
    }
}

// ---- Graph -----------------------------------------------------------------

/// The trust band's colour: red below 0.4, amber to 0.6, green above.
fn band(e: f32) -> Color {
    if e < 0.4 {
        red()
    } else if e <= 0.6 {
        amber()
    } else {
        green()
    }
}

fn name_of(a: &Agent) -> String {
    if a.name.trim().is_empty() {
        "Unnamed".to_string()
    } else {
        a.name.clone()
    }
}

/// Maps the layout's box onto the canvas: the whole box, fitted and centred (the web's
/// `preserveAspectRatio` meet).
#[derive(Clone, Copy)]
struct Xf {
    s: f32,
    dx: f32,
    dy: f32,
}

impl Xf {
    fn fit(size: Size) -> Self {
        let s = (size.width / LW).min(size.height / LH).max(0.01);
        Self {
            s,
            dx: (size.width - LW * s) / 2.0,
            dy: (size.height - LH * s) / 2.0,
        }
    }

    fn map(&self, p: Pt) -> Point {
        Point::new(p.x * self.s + self.dx, p.y * self.s + self.dy)
    }
}

/// A label cut to 16 characters.
fn clip(name: &str) -> String {
    if name.chars().count() > 16 {
        name.chars().take(15).chain(std::iter::once('…')).collect()
    } else {
        name.to_string()
    }
}

/// The name tag's text for an agent.
fn label_of(a: &Agent) -> String {
    clip(&name_of(a))
}

/// The width in pixels of a label drawn in the mono label size, with its padding.
fn label_width(label: &str) -> f32 {
    label.chars().count() as f32 * LABEL_CHAR + 2.0 * LABEL_PAD
}

/// The name tag's width in pixels for a label.
fn tag_width(label: &str) -> f32 {
    label_width(label).max(56.0)
}

/// A curve from a's edge to z's edge, bowed to one side. Reversed bonds bow apart.
fn edge_curve(a: Pt, z: Pt, ra: f32, rz: f32) -> (Pt, Pt, Pt) {
    let (dx, dy) = (z.x - a.x, z.y - a.y);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let (ux, uy) = (dx / len, dy / len);
    let p0 = Pt {
        x: a.x + ux * ra,
        y: a.y + uy * ra,
    };
    let p1 = Pt {
        x: z.x - ux * rz,
        y: z.y - uy * rz,
    };
    let c = Pt {
        x: (p0.x + p1.x) / 2.0 - uy * len * 0.16,
        y: (p0.y + p1.y) / 2.0 + ux * len * 0.16,
    };
    (p0, c, p1)
}

/// How far a node's edges start from its centre: the disc's edge, or a box's.
fn radius(n: &Node) -> f32 {
    match n.key {
        Key::Agent(_) => DISC,
        _ => n.w.min(n.h) / 2.0,
    }
}

/// The pop in progress, as the graph draws it: `keys` sit at `origin` at t = 0 and at their spots at t = 1.
#[derive(Clone, Copy)]
struct Grow<'a> {
    origin: Pt,
    t: f32,
    keys: &'a HashSet<Key>,
}

struct Graph<'a> {
    scene: &'a Scene,
    pos: &'a HashMap<Key, Pt>,
    selected: Option<&'a Key>,
    grow: Option<Grow<'a>>,
    suspended: &'a HashSet<u64>,
}

impl Graph<'_> {
    /// Where a node is drawn, in world units.
    fn at(&self, key: &Key) -> Option<Pt> {
        let p = *self.pos.get(key)?;
        Some(match self.grow {
            Some(g) if g.t < 1.0 && g.keys.contains(key) => Pt {
                x: g.origin.x + (p.x - g.origin.x) * g.t,
                y: g.origin.y + (p.y - g.origin.y) * g.t,
            },
            _ => p,
        })
    }

    /// The node under a point of the canvas, if any: its body and its label, as its footprint.
    fn hit(&self, at: Point, size: Size) -> Option<Key> {
        let xf = Xf::fit(size);
        for n in self.scene.nodes.iter().rev() {
            let Some(p) = self.at(&n.key) else { continue };
            let c = xf.map(p);
            if (at.x - c.x).abs() <= n.fp.half * xf.s
                && at.y >= c.y - n.fp.up * xf.s
                && at.y <= c.y + n.fp.down * xf.s
            {
                return Some(n.key.clone());
            }
        }
        None
    }
}

impl canvas::Program<Msg> for Graph<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut (),
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Msg>> {
        let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else {
            return None;
        };
        let at = cursor.position_in(bounds)?;
        // A click on an agent selects it, on a cluster pops it open, and on the background clears the selection.
        let msg = match self.hit(at, bounds.size()) {
            Some(key @ (Key::Family(_) | Key::Role(..))) => Msg::Open(key),
            key => Msg::Pick(key),
        };
        Some(canvas::Action::publish(msg).and_capture())
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let p = crate::theme::palette();
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let xf = Xf::fit(bounds.size());
        let k = xf.s;
        let nodes: HashMap<&Key, &Node> = self.scene.nodes.iter().map(|n| (&n.key, n)).collect();
        // An agent selection lights the node it is drawn in; a group selection lights its members.
        let sel_node: Option<&Key> = match self.selected {
            Some(Key::Agent(cid)) => self
                .scene
                .nodes
                .iter()
                .find(|n| n.members.contains(cid))
                .map(|n| &n.key),
            _ => None,
        };
        let group: Option<HashSet<u64>> = self
            .selected
            .filter(|key| !matches!(key, Key::Agent(_)))
            .map(|key| {
                self.scene
                    .nodes
                    .iter()
                    .find(|n| &n.key == key)
                    .map(|n| n.members.iter().copied().collect())
                    .unwrap_or_default()
            });
        let in_group = |n: &Node| {
            group
                .as_ref()
                .is_some_and(|g| n.members.iter().all(|c| g.contains(c)))
        };
        // The links the selection lights: a group's crossing contacts, or an agent's own.
        let lit = |l: &Link| -> bool {
            if group.is_some() {
                let a = nodes.get(&l.from).is_some_and(|n| in_group(n));
                let z = nodes.get(&l.to).is_some_and(|n| in_group(n));
                a != z
            } else {
                sel_node.is_some_and(|s| l.from == *s || l.to == *s)
            }
        };
        // The selection's nodes stay bright, with the far end of each lit contact; the rest dim.
        let mut bright: HashSet<&Key> = HashSet::new();
        if let Some(key) = self.selected {
            bright.insert(key);
            if let Some(s) = sel_node {
                bright.insert(s);
            }
            for n in &self.scene.nodes {
                if in_group(n) {
                    bright.insert(&n.key);
                }
            }
            for l in &self.scene.links {
                if lit(l) {
                    bright.insert(&l.from);
                    bright.insert(&l.to);
                }
            }
        }

        // Links first, so the nodes sit on top of them. Their colour is the trust band, the mutual
        // pairs are dark grey; the selection lights its own, and fades the rest.
        for l in &self.scene.links {
            let (Some(from), Some(to), Some(a), Some(z)) = (
                self.at(&l.from),
                self.at(&l.to),
                nodes.get(&l.from),
                nodes.get(&l.to),
            ) else {
                continue;
            };
            let (p0, c, p1) = edge_curve(from, to, radius(a), radius(z));
            let (s0, sc, s1) = (xf.map(p0), xf.map(c), xf.map(p1));
            let path = canvas::Path::new(|pb| {
                pb.move_to(s0);
                pb.quadratic_curve_to(sc, s1);
            });
            let base = if l.mutual { p.ink_2 } else { band(l.e) };
            let base_width =
                1.25 + 0.75 * clamp(l.strength / if l.directed { 10.0 } else { 30.0 }, 0.0, 1.0);
            let (colour, width, dashed, arrow) = if self.selected.is_none() {
                (base.scale_alpha(0.55), base_width, false, false)
            } else if lit(l) {
                let out = l.directed && sel_node.is_some_and(|s| l.from == *s);
                (base, 2.0, l.directed && !out, out)
            } else {
                (base.scale_alpha(0.12), base_width, false, false)
            };
            let mut stroke = canvas::Stroke::default()
                .with_color(colour)
                .with_width(width * k.max(0.5));
            if dashed {
                stroke = canvas::Stroke {
                    line_dash: canvas::stroke::LineDash {
                        segments: &[6.0, 4.0],
                        offset: 0,
                    },
                    ..stroke
                };
            }
            frame.stroke(&path, stroke);
            if arrow {
                // The arrowhead points into the peer, at the node's edge.
                let (dx, dy) = (s1.x - sc.x, s1.y - sc.y);
                let len = (dx * dx + dy * dy).sqrt().max(0.001);
                let (ux, uy) = (dx / len, dy / len);
                let base_pt = Point::new(s1.x - ux * 10.0 * k, s1.y - uy * 10.0 * k);
                let (nx, ny) = (-uy * 5.0 * k, ux * 5.0 * k);
                let head = canvas::Path::new(|pb| {
                    pb.move_to(s1);
                    pb.line_to(Point::new(base_pt.x + nx, base_pt.y + ny));
                    pb.line_to(Point::new(base_pt.x - nx, base_pt.y - ny));
                    pb.close();
                });
                frame.fill(&head, band(l.e));
            }
        }

        // Nodes: the agents as discs, the groups as boxes.
        for n in &self.scene.nodes {
            let Some(pt) = self.at(&n.key) else { continue };
            let c = xf.map(pt);
            let selected = self.selected == Some(&n.key);
            let alpha = if self.selected.is_some() && !bright.contains(&n.key) {
                0.35
            } else {
                1.0
            };
            let suspended = n.members.iter().any(|cid| self.suspended.contains(cid));
            match &n.key {
                Key::Agent(_) => draw_agent(&mut frame, n, c, k, alpha, selected, suspended),
                _ => draw_group(&mut frame, n, c, k, alpha, selected, suspended),
            }
        }
        vec![frame.into_geometry()]
    }
}

/// The initials on an agent's disc: the first letters of its first two words, or its first two letters.
fn initials(label: &str) -> String {
    let words: Vec<&str> = label.split_whitespace().collect();
    if words.len() > 1 {
        words
            .iter()
            .take(2)
            .filter_map(|w| w.chars().next())
            .collect::<String>()
            .to_uppercase()
    } else {
        label.chars().take(2).collect::<String>().to_uppercase()
    }
}

/// One line of canvas text: where it sits, how it is set, and whether it has the panel's halo, which
/// the web's labels carry so that they read over the lines.
struct Line<'s> {
    text: &'s str,
    at: Point,
    colour: Color,
    size: f32,
    font: iced::Font,
    x: iced::widget::text::Alignment,
    y: iced::alignment::Vertical,
    halo: bool,
}

fn line(frame: &mut canvas::Frame, l: Line<'_>) {
    let mut put = |at: Point, colour: Color| {
        frame.fill_text(canvas::Text {
            content: l.text.to_string(),
            position: at,
            color: colour,
            size: l.size.into(),
            font: l.font,
            align_x: l.x,
            align_y: l.y,
            ..canvas::Text::default()
        });
    };
    if l.halo {
        let panel = crate::theme::palette().panel;
        for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            put(Point::new(l.at.x + dx, l.at.y + dy), panel);
        }
    }
    put(l.at, l.colour);
}

/// An agent: its disc with its initials, its name under it, and a dot when it is suspended. The
/// selected agent's disc takes the signal's ring, as the web's does.
fn draw_agent(
    frame: &mut canvas::Frame,
    n: &Node,
    c: Point,
    k: f32,
    alpha: f32,
    selected: bool,
    suspended: bool,
) {
    let p = crate::theme::palette();
    let disc = canvas::Path::circle(c, DISC * k);
    frame.fill(&disc, p.raised.scale_alpha(alpha));
    let (colour, width) = if selected {
        (p.signal, 2.5)
    } else {
        (p.rule_strong, 1.5)
    };
    frame.stroke(
        &disc,
        canvas::Stroke::default()
            .with_color(colour.scale_alpha(alpha))
            .with_width(width * k),
    );
    line(
        frame,
        Line {
            text: &initials(&n.label),
            at: c,
            colour: p.ink.scale_alpha(alpha),
            size: 12.0,
            font: crate::theme::SANS_SEMIBOLD,
            x: iced::widget::text::Alignment::Center,
            y: iced::alignment::Vertical::Center,
            halo: false,
        },
    );
    line(
        frame,
        Line {
            text: &n.label,
            at: Point::new(c.x, c.y + (DISC + 6.0) * k),
            colour: p.ink_2.scale_alpha(alpha),
            size: LABEL_SIZE,
            font: crate::theme::SANS,
            x: iced::widget::text::Alignment::Center,
            y: iced::alignment::Vertical::Top,
            halo: true,
        },
    );
    if suspended {
        suspended_dot(
            frame,
            Point::new(c.x + DISC * 0.7 * k, c.y - DISC * 0.7 * k),
            k,
            alpha,
        );
    }
}

/// The dot a suspended agent carries: a danger-coloured disc at its corner.
fn suspended_dot(frame: &mut canvas::Frame, at: Point, k: f32, alpha: f32) {
    let dot = canvas::Path::circle(at, 5.0 * k);
    frame.fill(&dot, Tone::Danger.color().scale_alpha(alpha));
}

/// A group's box: a rounded square, dashed, with up to three of its members as chips and its count
/// in the middle; its name under it. The selected group's edge takes the signal.
fn draw_group(
    frame: &mut canvas::Frame,
    n: &Node,
    c: Point,
    k: f32,
    alpha: f32,
    selected: bool,
    suspended: bool,
) {
    let p = crate::theme::palette();
    let side = n.w * k;
    let half = side / 2.0;
    let (x0, y0) = (c.x - half, c.y - half);
    let body = canvas::Path::rounded_rectangle(
        Point::new(x0, y0),
        Size::new(side, side),
        border::Radius::from(12.0 * k),
    );
    frame.fill(&body, p.sheet_2.scale_alpha(alpha));
    let (colour, width) = if selected {
        (p.signal, 2.5)
    } else {
        (p.rule_strong, 1.0)
    };
    let dashed = canvas::Stroke {
        line_dash: canvas::stroke::LineDash {
            segments: &[4.0, 3.0],
            offset: 0,
        },
        ..canvas::Stroke::default()
    };
    frame.stroke(
        &body,
        dashed
            .with_color(colour.scale_alpha(alpha))
            .with_width(width * k.max(0.5)),
    );
    for i in 0..n.members.len().min(3) {
        let chip = canvas::Path::rounded_rectangle(
            Point::new(x0 + (8.0 + i as f32 * 14.0) * k, y0 + 8.0 * k),
            Size::new(10.0 * k, 10.0 * k),
            border::Radius::from(2.0 * k),
        );
        frame.fill(&chip, p.signal_soft.scale_alpha(alpha));
    }
    line(
        frame,
        Line {
            text: &n.members.len().to_string(),
            at: c,
            colour: p.ink_2.scale_alpha(alpha),
            size: 12.0,
            font: crate::theme::SANS,
            x: iced::widget::text::Alignment::Center,
            y: iced::alignment::Vertical::Center,
            halo: false,
        },
    );
    line(
        frame,
        Line {
            text: &n.label,
            at: Point::new(c.x, c.y + half + 6.0 * k),
            colour: p.ink_2.scale_alpha(alpha),
            size: LABEL_SIZE,
            font: crate::theme::SANS,
            x: iced::widget::text::Alignment::Center,
            y: iced::alignment::Vertical::Top,
            halo: true,
        },
    );
    if suspended {
        suspended_dot(
            frame,
            Point::new(x0 + side - 4.0 * k, y0 + 4.0 * k),
            k,
            alpha,
        );
    }
}

// ---- Breadcrumb and controls -----------------------------------------------

/// The controls above the graph (the web's `.soc-toolbar`): the family toggle, and Back when a family is open.
fn toolbar<'a>(grouped: bool, depth: usize) -> Element<'a, Msg> {
    let mut bar = row![ui::btn_sm(
        "Group by family",
        None,
        Kind::Secondary,
        Some(Msg::Group)
    )]
    .spacing(12);
    if grouped && depth > 0 {
        bar = bar.push(ui::btn_sm(
            "Back",
            None,
            Kind::Secondary,
            Some(Msg::Level(depth - 1)),
        ));
    }
    bar.align_y(Alignment::Center).into()
}

/// The breadcrumb, "All families  Build  builder" (the web's `.soc-crumbs`): each crumb but the last one climbs back to it.
fn crumbs<'a>(nav: &[Key]) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![crumb("All families", 0, nav.is_empty())];
    for (i, key) in nav.iter().enumerate() {
        let label = match key {
            Key::Family(f) => family_label(f).to_string(),
            Key::Role(_, r) => r.clone(),
            Key::Agent(_) => String::new(),
        };
        parts.push(crumb(&label, i + 1, i + 1 == nav.len()));
    }
    row(parts).spacing(12).align_y(Alignment::Center).into()
}

fn crumb<'a>(label: &str, level: usize, current: bool) -> Element<'a, Msg> {
    if current {
        ui::sans(label.to_string(), 12.0, crate::theme::palette().ink).into()
    } else {
        ui::btn_sm(label, None, Kind::Ghost, Some(Msg::Level(level)))
    }
}

/// A card's surface for the graph and the panel (the web's `.soc-graph`, `.soc-panel`): the sheet, ruled, the
/// skin's large radius, no shadow.
fn surface(_: &Theme) -> container::Style {
    container::Style {
        text_color: Some(crate::theme::palette().ink),
        background: Some(crate::theme::palette().sheet.into()),
        border: border::Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: crate::theme::skin().radius_lg.into(),
        },
        ..container::Style::default()
    }
}

/// A heading in the panel (the web's `h2`, `h3`): semibold sans at `size`.
fn heading<'a>(content: &str, size: f32) -> text::Text<'a> {
    text(content.to_string())
        .font(crate::theme::SANS_SEMIBOLD)
        .size(size)
        .color(crate::theme::palette().ink)
}

/// A panel's section heading (the web's `h3`).
fn h3<'a>(content: &str) -> Element<'a, Msg> {
    heading(content, 13.0).into()
}

// ---- Panel -----------------------------------------------------------------

/// The bond from `from` to `to`, if there is one.
fn bond_between(d: &Society, from: u64, to: u64) -> Option<&Bond> {
    d.bonds.iter().find(|b| b.from == from && b.to == to)
}

/// The panel's head (the web's `viewHead`): the title, and under it the subtitle.
fn panel_head<'a>(title: String, sub: String, extra: Option<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut line = row![heading(&title, 18.0)]
        .spacing(8)
        .align_y(Alignment::Center);
    if let Some(extra) = extra {
        line = line.push(extra);
    }
    column![line, ui::sans(sub, 13.0, crate::theme::palette().ink_3)]
        .spacing(4)
        .into()
}

/// The selected agent's panel: its own reputation and contacts, both ways round.
fn detail<'a>(state: &'a State, d: &'a Society, me: &'a Agent) -> Element<'a, Msg> {
    let names: HashMap<u64, &Agent> = d.agents.iter().map(|a| (a.cid, a)).collect();
    let name = name_of(me);
    let rep = clamp(me.reputation, 0.0, 100.0);
    let rated = me.confidence >= 0.05;

    // Two holders disagree when their opinions of each other differ by more than DISAGREE.
    let gap = |peer: u64| -> bool {
        match (bond_between(d, me.cid, peer), bond_between(d, peer, me.cid)) {
            (Some(ab), Some(ba)) => (ab.opinion.e - ba.opinion.e).abs() > DISAGREE,
            _ => false,
        }
    };
    let peer_row = |peer: u64, bond: &'a Bond| -> Element<'a, Msg> {
        let peer_name = names
            .get(&peer)
            .map(|a| name_of(a))
            .unwrap_or_else(|| format!("agent {peer}"));
        let (kind_tone, kind) = match bond.kind.as_str() {
            "hired" => (Tone::Signal, "hired"),
            "was-hired" => (Tone::Info, "was-hired"),
            "added" => (Tone::Ok, "added"),
            _ => (Tone::Muted, "met"),
        };
        let mut names_row = row![
            ui::btn_sm(
                &peer_name,
                None,
                Kind::Ghost,
                Some(Msg::Pick(Some(Key::Agent(peer))))
            ),
            ui::badge(kind, kind_tone)
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if gap(peer) {
            names_row = names_row.push(ui::badge("they disagree", Tone::Warn));
        }
        let note = bond
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| note_view(n, peer, state.open_notes.contains(&peer)));
        peer_entry(names_row.into(), &bond.opinion, note)
    };

    let outgoing: Vec<Element<'a, Msg>> = d
        .bonds
        .iter()
        .filter(|b| b.from == me.cid && names.contains_key(&b.to))
        .map(|b| peer_row(b.to, b))
        .collect();
    let incoming: Vec<Element<'a, Msg>> = d
        .bonds
        .iter()
        .filter(|b| b.to == me.cid && names.contains_key(&b.from))
        .map(|b| peer_row(b.from, b))
        .collect();

    let suspended = state
        .suspended
        .contains(&me.cid)
        .then(|| ui::badge("suspended", Tone::Danger));
    let rep_meter = meter(if rated { rep / 100.0 } else { 0.0 });
    let mut panel = column![
        panel_head(name.clone(), role_label(&me.role), suspended),
        stats(vec![
            stat_box(
                "Reputation",
                if rated {
                    format!("{} / 100", rep.round())
                } else {
                    "Unrated".to_string()
                },
                Some(rep_meter),
            ),
            stat_box(
                "Confidence",
                format!("{}%", (clamp(me.confidence, 0.0, 1.0) * 100.0).round()),
                None,
            ),
            stat_box("Wallet", compact(me.wallet), None),
        ]),
        row![
            ui::btn_sm(
                "Trust more",
                None,
                Kind::Primary,
                Some(Msg::Rate(me.cid, 1.0))
            ),
            ui::btn_sm(
                "Trust less",
                None,
                Kind::Secondary,
                Some(Msg::Rate(me.cid, -1.0))
            ),
        ]
        .spacing(8),
        h3(&format!("How {name} sees others")),
    ]
    .spacing(12);
    let outgoing_list: Element<'a, Msg> = if outgoing.is_empty() {
        ui::faint("No contacts yet.").into()
    } else {
        column(outgoing).into()
    };
    let incoming_list: Element<'a, Msg> = if incoming.is_empty() {
        ui::faint("Nobody has met them yet.").into()
    } else {
        column(incoming).into()
    };
    panel = panel.push(outgoing_list);
    panel = panel.push(h3(&format!("How others see {name}")));
    panel = panel.push(incoming_list);

    let mut out: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        out.push(ui::error_line(e));
    }
    out.push(panel.into());
    column(out).spacing(12).into()
}

/// A family's or a role's panel (the web's `groupDetail`): its members, the trust that runs in and out
/// of it, and its strongest outside ties.
fn group_panel<'a>(d: &'a Society, key: &Key) -> Element<'a, Msg> {
    let members: Vec<&Agent> = d.agents.iter().filter(|a| in_group(a, key)).collect();
    let inside: HashSet<u64> = members.iter().map(|a| a.cid).collect();
    let names: HashMap<u64, &Agent> = d.agents.iter().map(|a| (a.cid, a)).collect();
    let outside: Vec<&Bond> = d
        .bonds
        .iter()
        .filter(|b| inside.contains(&b.from) != inside.contains(&b.to))
        .collect();
    let mean = |list: Vec<f32>| -> Option<f32> {
        (!list.is_empty()).then(|| list.iter().sum::<f32>() / list.len() as f32)
    };
    let trust_in = mean(
        outside
            .iter()
            .filter(|b| inside.contains(&b.to))
            .map(|b| b.opinion.e)
            .collect(),
    );
    let trust_out = mean(
        outside
            .iter()
            .filter(|b| inside.contains(&b.from))
            .map(|b| b.opinion.e)
            .collect(),
    );
    let mut ties = outside.clone();
    ties.sort_by(|a, b| b.strength.total_cmp(&a.strength));

    let (title, sub) = match key {
        Key::Family(f) => (
            family_label(f).to_string(),
            format!("{} agents", members.len()),
        ),
        Key::Role(f, r) => (
            r.clone(),
            format!("{} agents · {}", members.len(), family_label(f)),
        ),
        Key::Agent(_) => (String::new(), String::new()),
    };
    let member_list: Element<'a, Msg> = if members.is_empty() {
        ui::faint("No agents here.").into()
    } else {
        column(
            members
                .iter()
                .map(|a| member_row(name_of(a), role_label(&a.role), Msg::Member(a.cid)))
                .collect::<Vec<Element<'a, Msg>>>(),
        )
        .into()
    };
    let tie_list: Vec<Element<'a, Msg>> = ties
        .iter()
        .take(3)
        .map(|b| -> Element<'a, Msg> {
            let peer = if inside.contains(&b.from) {
                b.to
            } else {
                b.from
            };
            let peer_name = names
                .get(&peer)
                .map(|a| name_of(a))
                .unwrap_or_else(|| format!("agent {peer}"));
            let head = row![
                ui::btn_sm(&peer_name, None, Kind::Ghost, Some(Msg::Member(peer))),
                ui::badge(&b.kind, Tone::Muted)
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            let note = b
                .note
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(|n| ui::sans(n.to_string(), 12.5, crate::theme::palette().ink_2).into());
            peer_entry(head.into(), &b.opinion, note)
        })
        .collect();
    let ties_col: Element<'a, Msg> = if tie_list.is_empty() {
        ui::faint("No contacts across the edge.").into()
    } else {
        column(tie_list).into()
    };

    column![
        panel_head(title, sub, None),
        stats(vec![
            stat_box(
                "Mean trust in",
                shown(trust_in),
                trust_in.map(|e| bar(&Opinion { e, u: 0.0 }))
            ),
            stat_box(
                "Mean trust out",
                shown(trust_out),
                trust_out.map(|e| bar(&Opinion { e, u: 0.0 }))
            ),
        ]),
        h3("Members"),
        member_list,
        h3("Strongest outside ties"),
        ties_col,
    ]
    .spacing(12)
    .into()
}

/// A mean trust, to two places; "none yet" when there are no bonds.
fn shown(v: Option<f32>) -> String {
    v.map(|e| format!("{e:.2}"))
        .unwrap_or_else(|| "none yet".to_string())
}

/// One peer's entry in a panel: the name and its badges, the opinion on the right, the opinion's bar,
/// then the private note when there is one.
fn peer_entry<'a>(
    head: Element<'a, Msg>,
    opinion: &Opinion,
    note: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let num = ui::sans(
        format!("{:.2}", opinion.e),
        12.0,
        crate::theme::palette().ink_2,
    );
    let top = row![container(head).width(Length::Fill), num]
        .spacing(12)
        .align_y(Alignment::Center);
    let mut item = column![top, bar(opinion)].spacing(8);
    if let Some(note) = note {
        item = item.push(note);
    }
    column![
        container(item).padding([12.0, 0.0]).width(Length::Fill),
        ui::hairline()
    ]
    .into()
}

/// A private note, clamped to NOTE_PREVIEW characters; "Show all" opens it, and "Show less" closes it.
fn note_view<'a>(note: &str, peer: u64, open: bool) -> Element<'a, Msg> {
    let long = note.chars().count() > NOTE_PREVIEW;
    let shown = if long && !open {
        format!("{}…", note.chars().take(NOTE_PREVIEW).collect::<String>())
    } else {
        note.to_string()
    };
    let text = ui::sans(shown, 12.5, crate::theme::palette().ink_2);
    if !long {
        return text.into();
    }
    let toggle = ui::btn_sm(
        if open { "Show less" } else { "Show all" },
        None,
        Kind::Ghost,
        Some(Msg::Note(peer)),
    );
    column![text, toggle].spacing(4).into()
}

/// A group's member: the name as a ghost button, and its role on the right.
fn member_row<'a>(name: String, role: String, msg: Msg) -> Element<'a, Msg> {
    let line = row![
        ui::btn_sm(&name, None, Kind::Ghost, Some(msg)),
        Space::new().width(Length::Fill),
        ui::faint(role),
    ]
    .align_y(Alignment::Center);
    column![
        container(line).padding([12.0, 0.0]).width(Length::Fill),
        ui::hairline()
    ]
    .into()
}

/// Three stat boxes in a row (the web's `.soc-stats`, three columns); fewer leave the rest empty.
fn stats<'a>(cells: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut slots: Vec<Element<'a, Msg>> = cells
        .into_iter()
        .map(|c| container(c).width(Length::FillPortion(1)).into())
        .collect();
    while slots.len() < 3 {
        slots.push(Space::new().width(Length::FillPortion(1)).into());
    }
    row(slots).spacing(12).into()
}

/// A stat box (the web's `.stat`): its label in ink-3, its value, and a meter under it when it has one.
fn stat_box<'a>(label: &str, value: String, extra: Option<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut body = column![ui::faint(label.to_string()), heading(&value, 13.5)].spacing(4);
    if let Some(extra) = extra {
        body = body.push(extra);
    }
    container(body)
        .padding([14.0, 16.0])
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().raised.into()),
            border: border::Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: crate::theme::skin().radius_lg.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// Reputation as a bar, filled to `frac` in the ok colour (the web's `.soc-bar` with `.t-ok`).
fn meter<'a>(frac: f32) -> Element<'a, Msg> {
    track(frac, 0.0, green())
}

/// An opinion (the web's `.soc-bar`): the filled part is the expected trust e; the hatched tail runs on by u.
fn bar<'a>(o: &Opinion) -> Element<'a, Msg> {
    let e = clamp(o.e, 0.0, 1.0);
    track(e, clamp(o.u, 0.0, 1.0).min(1.0 - e), band(e))
}

/// A track of `width` fraction filled in `colour`, then a faint run of `tail` fraction.
fn track<'a>(width: f32, tail: f32, colour: Color) -> Element<'a, Msg> {
    let fill = permille(width);
    let tail_w = permille(tail).min(1000 - fill);
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if fill > 0 {
        parts.push(tint(Length::FillPortion(fill), colour));
    }
    if tail_w > 0 {
        parts.push(tint(
            Length::FillPortion(tail_w),
            crate::theme::palette().ink_3.scale_alpha(0.3),
        ));
    }
    if fill + tail_w < 1000 {
        parts.push(
            Space::new()
                .width(Length::FillPortion(1000 - fill - tail_w))
                .height(Length::Fill)
                .into(),
        );
    }
    container(row(parts).height(Length::Fill))
        .width(Length::Fill)
        .height(8.0)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().rule.into()),
            border: border::Border {
                radius: 4.0.into(),
                ..border::Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// The page with nothing selected (the web's `legend`): how to read the graph.
fn legend<'a>() -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let swatch =
        |colour: Color, height: f32| -> Element<'a, Msg> { tint_sized(28.0, height, colour) };
    let dashed = || -> Element<'a, Msg> {
        row![
            tint_sized(6.0, 4.0, p.ink_3),
            tint_sized(6.0, 4.0, p.ink_3),
            tint_sized(6.0, 4.0, p.ink_3)
        ]
        .spacing(4)
        .into()
    };
    let entry = |sw: Element<'a, Msg>, text: &'static str| -> Element<'a, Msg> {
        row![
            container(sw).width(28.0).center_y(Length::Shrink),
            ui::sans(text, 13.0, p.ink_2)
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    };
    let boxed =
        container(Space::new().width(14.0).height(14.0)).style(|_: &Theme| container::Style {
            border: border::Border {
                color: crate::theme::palette().rule_strong,
                width: 1.0,
                radius: 3.0.into(),
            },
            ..container::Style::default()
        });
    column![
        panel_head(
            "Nothing selected".to_string(),
            "Pick an agent to see how it sees the others, and how they see it.".to_string(),
            None,
        ),
        h3("Reading the graph"),
        column![
            entry(swatch(red(), 4.0), "trust below 0.4"),
            entry(swatch(amber(), 4.0), "trust from 0.4 to 0.6"),
            entry(swatch(green(), 4.0), "trust above 0.6"),
            entry(
                swatch(p.ink_3, 4.0),
                "solid, with an arrowhead: its own opinions"
            ),
            entry(dashed(), "dashed: the opinions held of it"),
            entry(swatch(p.ink_3, 8.0), "thicker line: a stronger contact"),
            entry(swatch(p.ink_2, 4.0), "darker grey: a mutual pair"),
            entry(
                boxed.into(),
                "a box: a group of agents, sized by its count; click to open it",
            ),
        ]
        .spacing(10),
        ui::sans(
            "Trusting an agent more or less moves the opinions held of it.",
            11.5,
            p.ink_3,
        ),
    ]
    .spacing(12)
    .into()
}

// ---- Small pieces ----------------------------------------------------------

/// A share of a bar in thousandths, so a fraction can be a FillPortion.
fn permille(frac: f32) -> u16 {
    (clamp(frac, 0.0, 1.0) * 1000.0).round() as u16
}

/// A solid strip filling its portion of a row.
fn tint<'a>(width: Length, colour: Color) -> Element<'a, Msg> {
    container(Space::new().width(width).height(Length::Fill))
        .style(move |_: &Theme| filled(colour))
        .into()
}

fn tint_sized<'a>(width: f32, height: f32, colour: Color) -> Element<'a, Msg> {
    container(Space::new().width(width).height(height))
        .style(move |_: &Theme| filled(colour))
        .into()
}

fn filled(colour: Color) -> container::Style {
    container::Style {
        background: Some(Background::Color(colour)),
        ..container::Style::default()
    }
}

/// A wallet figure, shortened: 1.2k, 3.4M.
fn compact(value: f64) -> String {
    let v = value.abs();
    let sign = if value < 0.0 { "-" } else { "" };
    if v >= 1_000_000.0 {
        format!("{sign}{:.1}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{sign}{:.1}k", v / 1_000.0)
    } else {
        format!("{sign}{:.0}", v)
    }
}

/// Esc goes up one level of the groups, as the web's does.
pub fn subscription() -> iced::Subscription<Msg> {
    iced::event::listen_with(|event, _status, _window| match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            ..
        }) => Some(Msg::Up),
        _ => None,
    })
}
