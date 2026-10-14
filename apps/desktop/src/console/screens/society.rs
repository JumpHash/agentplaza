//! The Society screen (parity with the web console's `society` view): the agents as a graph of
//! who knows whom, drawn on a canvas, and the selected agent's opinions in both directions.
//!
//! Above `GROUP_AT` agents the graph opens grouped by family: a family is a cluster node, a click
//! pops it open into its roles (a role with one agent shows that agent), and a role pops open into
//! its agents. The breadcrumb climbs back up, and the toggle switches between the family level and
//! the agent level at any size.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use iced::widget::{Space, canvas, column, container, mouse_area, row};
use iced::{
    Alignment, Background, Color, Element, Length, Point, Rectangle, Renderer, Size, Task, Theme,
    border, mouse,
};
use serde::Deserialize;
use serde_json::Value;

use super::{Id, sanctions};
use crate::console::engine::world::layout::agent_hue;
use crate::console::ui::{self, Kind, Tone};
use crate::console::{Ctx, client};

/// The layout's world box. The first layout spreads the agents over all of it, and the view maps
/// the box onto the graph area, so the graph grows and shrinks with the panel.
const LW: f32 = 760.0;
const LH: f32 = 700.0;
/// The least scale the graph is drawn at. Name tags are a fixed size in pixels, so the room they
/// take is reserved in world units at this scale.
const TAG_SCALE: f32 = 0.7;
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
/// Name tags and cluster labels are mono text of this size, in pixels, with this padding.
const LABEL_SIZE: f32 = 13.0;
/// One mono character at `LABEL_SIZE`, a little generous so that no glyph is cut off.
const LABEL_CHAR: f32 = 0.7 * LABEL_SIZE;
const LABEL_PAD: f32 = 8.0;
/// The pop: how long the nodes that come into view take to grow out of the ones that went, and the
/// step between frames.
const POP: Duration = Duration::from_millis(250);
const FRAME: Duration = Duration::from_millis(16);
const RED: Color = Color::from_rgb8(0xe5, 0x53, 0x3d);
const AMBER: Color = Color::from_rgb8(0xf2, 0xb8, 0x4b);
const GREEN: Color = Color::from_rgb8(0x3f, 0xae, 0x6a);

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
                Key::Family(f) => vec![Key::Family(*f)],
                Key::Role(f, r) => vec![Key::Family(*f), Key::Role(*f, r.clone())],
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
            let graph = Graph {
                scene: &state.scene,
                pos: &state.pos,
                selected: state.selected.as_ref(),
                grow,
                suspended: &state.suspended,
            };
            let panel = match &state.selected {
                Some(Key::Agent(cid)) => match d.agents.iter().find(|a| a.cid == *cid) {
                    Some(me) => detail(state, d, me),
                    None => legend(),
                },
                Some(key) if d.agents.iter().any(|a| in_group(a, key)) => group_panel(d, key),
                _ => legend(),
            };
            let mut left = column![toolbar(grouped, state.nav.len())];
            if grouped {
                left = left.push(crumbs(&state.nav));
            }
            left = left.push(canvas(graph).width(Length::Fill).height(Length::Fill));
            row![
                container(left)
                    .width(Length::FillPortion(3))
                    .height(Length::Fill),
                container(
                    ui::scroll(column![panel])
                        .width(Length::Fill)
                        .height(Length::Fill)
                )
                .width(Length::FillPortion(2))
                .height(Length::Fill),
            ]
            .spacing(16)
            .height(Length::Fill)
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
        let s = side(a.reputation);
        return Node {
            key: key.clone(),
            label: label_of(a),
            members: vec![a.cid],
            w: s,
            h: s,
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
    // Sized by member count on a square root scale, and wide enough for its label.
    let size = 52.0 + 10.0 * (cids.len() as f32).sqrt();
    let w = size.max(label_width(&label));
    let h = size.max(60.0);
    Node {
        key: key.clone(),
        label,
        members: cids,
        w,
        h,
        fp: Foot {
            half: w / 2.0,
            up: h / 2.0 + 8.0,
            down: h / 2.0 + 8.0,
        },
    }
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
    let half = side(a.reputation) / 2.0;
    let tag = tag_width(&label_of(a));
    Foot {
        half: half.max(tag / (2.0 * TAG_SCALE)),
        up: half + 18.0,
        down: half + 8.0 + 22.0 / TAG_SCALE,
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
        for (n, &i) in fresh.iter().enumerate() {
            let angle = n as f32 * 2.39996; // golden angle, so a crowd fans out evenly
            let r = 70.0 + 26.0 * (n as f32).sqrt();
            pos.insert(
                nodes[i].key.clone(),
                Pt {
                    x: LW / 2.0 + angle.cos() * r,
                    y: LH / 2.0 + angle.sin() * r,
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

    let steps = if fresh.len() == nodes.len() {
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
    if !placed {
        stretch(&mut bodies);
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

/// Stretches the nodes over the box, each axis on its own, so a first layout uses all of it.
/// A single row or column of nodes goes to the middle of its axis.
fn stretch(bodies: &mut [Body]) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for b in bodies.iter() {
        x0 = x0.min(b.x);
        y0 = y0.min(b.y);
        x1 = x1.max(b.x);
        y1 = y1.max(b.y);
    }
    let map = |v: f32, lo: f32, hi: f32, out_lo: f32, out_hi: f32| {
        if hi - lo < 1.0 {
            (out_lo + out_hi) / 2.0
        } else {
            out_lo + (v - lo) / (hi - lo) * (out_hi - out_lo)
        }
    };
    for b in bodies.iter_mut() {
        b.x = map(b.x, x0, x1, 60.0, LW - 60.0);
        b.y = map(b.y, y0, y1, 70.0, LH - 70.0);
    }
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

/// The node's side in world units: bigger with reputation.
fn side(reputation: f32) -> f32 {
    46.0 + 12.0 * clamp(reputation / 100.0, 0.0, 1.0)
}

/// The trust band's colour: red below 0.4, amber to 0.6, green above.
fn band(e: f32) -> Color {
    if e < 0.4 {
        RED
    } else if e <= 0.6 {
        AMBER
    } else {
        GREEN
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

/// How far a node's edges start from its centre.
fn radius(n: &Node) -> f32 {
    n.w.min(n.h) / 2.0 + 8.0
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

    /// The node under a point of the canvas, if any: its body, or for an agent its name tag too.
    fn hit(&self, at: Point, size: Size) -> Option<Key> {
        let xf = Xf::fit(size);
        for n in self.scene.nodes.iter().rev() {
            let Some(p) = self.at(&n.key) else { continue };
            let c = xf.map(p);
            let (up, down) = if matches!(n.key, Key::Agent(_)) {
                (16.0, 30.0)
            } else {
                (0.0, 0.0)
            };
            let (w, h) = match n.key {
                Key::Agent(_) => (n.w * xf.s, n.h * xf.s),
                _ => cluster_box(n, xf.s),
            };
            let (hw, hh) = (w / 2.0, h / 2.0);
            if (at.x - c.x).abs() <= hw && at.y >= c.y - hh - up && at.y <= c.y + hh + down {
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
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let xf = Xf::fit(bounds.size());
        let k = xf.s;
        let nodes: HashMap<&Key, &Node> = self.scene.nodes.iter().map(|n| (&n.key, n)).collect();
        // The nodes the selection has a link to.
        let touching: HashSet<&Key> = match self.selected {
            Some(sel) => self
                .scene
                .links
                .iter()
                .filter_map(|l| {
                    if &l.from == sel {
                        Some(&l.to)
                    } else if &l.to == sel {
                        Some(&l.from)
                    } else {
                        None
                    }
                })
                .collect(),
            None => HashSet::new(),
        };

        // Links first, so the nodes sit on top of them.
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
            let (colour, width, dashed, arrow) = if l.directed {
                match self.selected {
                    None => (
                        if l.mutual {
                            crate::theme::palette().ink_2
                        } else {
                            crate::theme::palette().ink_3
                        },
                        if l.mutual { 1.5 } else { 1.2 },
                        false,
                        false,
                    ),
                    Some(sel) if l.from == *sel || l.to == *sel => {
                        let out = l.from == *sel;
                        (
                            band(l.e),
                            1.5 + 4.0 * clamp(l.strength / 10.0, 0.0, 1.0),
                            !out,
                            out,
                        )
                    }
                    Some(_) => (
                        crate::theme::palette().ink_3.scale_alpha(0.25),
                        1.0,
                        false,
                        false,
                    ),
                }
            } else {
                // Aggregated: thicker with the summed strength, coloured by the mean trust.
                let touches = self
                    .selected
                    .is_some_and(|sel| l.from == *sel || l.to == *sel);
                let colour = if self.selected.is_some() && !touches {
                    band(l.e).scale_alpha(0.2)
                } else {
                    band(l.e)
                };
                (
                    colour,
                    1.0 + 4.0 * clamp(l.strength / 40.0, 0.0, 1.0),
                    false,
                    false,
                )
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
                // The arrowhead points into the holder's peer, at the node's edge.
                let (dx, dy) = (s1.x - sc.x, s1.y - sc.y);
                let len = (dx * dx + dy * dy).sqrt().max(0.001);
                let (ux, uy) = (dx / len, dy / len);
                let base = Point::new(s1.x - ux * 12.0, s1.y - uy * 12.0);
                let (nx, ny) = (-uy * 5.0, ux * 5.0);
                let head = canvas::Path::new(|pb| {
                    pb.move_to(s1);
                    pb.line_to(Point::new(base.x + nx, base.y + ny));
                    pb.line_to(Point::new(base.x - nx, base.y - ny));
                    pb.close();
                });
                frame.fill(&head, colour);
            }
        }

        // Nodes: the agents as robots, the clusters as labelled boxes.
        for n in &self.scene.nodes {
            let Some(p) = self.at(&n.key) else { continue };
            let c = xf.map(p);
            let selected = self.selected == Some(&n.key);
            let dim = self
                .selected
                .is_some_and(|sel| sel != &n.key && !touching.contains(&n.key));
            let alpha = if dim { 0.35 } else { 1.0 };
            match &n.key {
                Key::Agent(_) => draw_agent(
                    &mut frame,
                    n,
                    c,
                    k,
                    alpha,
                    selected,
                    self.suspended.contains(&n.members[0]),
                ),
                _ => draw_cluster(&mut frame, n, c, k, alpha, selected),
            }
        }
        vec![frame.into_geometry()]
    }
}

/// An agent: a square body with a head, two eyes, a mouth and a name tag. A suspended agent carries a
/// danger-coloured ring segment at the top-right corner of its body.
fn draw_agent(
    frame: &mut canvas::Frame,
    n: &Node,
    c: Point,
    k: f32,
    alpha: f32,
    selected: bool,
    suspended: bool,
) {
    let cid = n.members[0];
    let sz = n.w * k;
    let half = sz / 2.0;
    let hue = agent_hue(cid);
    let ink = crate::theme::palette().ink.scale_alpha(alpha);

    // The print shadow, the antenna and its tip.
    frame.fill_rectangle(
        Point::new(c.x - half + 3.0, c.y - half + 3.0),
        Size::new(sz, sz),
        crate::theme::palette().ink.scale_alpha(0.85 * alpha),
    );
    let antenna = canvas::Path::line(
        Point::new(c.x, c.y - half),
        Point::new(c.x, c.y - half - 12.0 * k),
    );
    frame.stroke(
        &antenna,
        canvas::Stroke::default()
            .with_color(ink)
            .with_width(3.0 * k),
    );
    let tip = canvas::Path::circle(Point::new(c.x, c.y - half - 13.0 * k), 4.5 * k);
    frame.fill(&tip, ui::hsl(hue, 0.7, 0.55).scale_alpha(alpha));
    frame.stroke(
        &tip,
        canvas::Stroke::default()
            .with_color(ink)
            .with_width(2.5 * k),
    );

    // The body, and the ring round the selected agent.
    let body = canvas::Path::rectangle(Point::new(c.x - half, c.y - half), Size::new(sz, sz));
    frame.fill(&body, ui::hsl(hue, 0.6, 0.64).scale_alpha(alpha));
    frame.stroke(
        &body,
        canvas::Stroke::default()
            .with_color(ink)
            .with_width(3.0 * k),
    );
    if selected {
        let ring = canvas::Path::rectangle(
            Point::new(c.x - half - 5.0, c.y - half - 5.0),
            Size::new(sz + 10.0, sz + 10.0),
        );
        frame.stroke(
            &ring,
            canvas::Stroke::default()
                .with_color(crate::theme::palette().signal)
                .with_width(2.0),
        );
    }
    if suspended {
        // A quarter ring centred on the body's top-right corner, outside the body: from straight up to straight right.
        let corner = Point::new(c.x + half, c.y - half);
        let arc = canvas::Path::new(|pb| {
            pb.arc(canvas::path::Arc {
                center: corner,
                radius: 9.0 * k,
                start_angle: (-std::f32::consts::FRAC_PI_2).into(),
                end_angle: 0.0_f32.into(),
            });
        });
        frame.stroke(
            &arc,
            canvas::Stroke::default()
                .with_color(Tone::Danger.color().scale_alpha(alpha))
                .with_width(3.0 * k)
                .with_line_cap(canvas::LineCap::Round),
        );
    }

    // Two eyes: a white sclera with an ink pupil each.
    for dir in [-1.0_f32, 1.0] {
        let ex = c.x + dir * sz * 0.2;
        let ey = c.y - sz * 0.08;
        let sclera = canvas::Path::circle(Point::new(ex, ey), sz * 0.14);
        frame.fill(&sclera, Color::WHITE.scale_alpha(alpha));
        frame.stroke(
            &sclera,
            canvas::Stroke::default()
                .with_color(ink)
                .with_width(2.0 * k),
        );
        let pupil = canvas::Path::circle(Point::new(ex + 1.5 * k, ey + 1.5 * k), sz * 0.06);
        frame.fill(&pupil, ink);
    }

    // The mouth.
    let mouth = canvas::Path::new(|pb| {
        pb.move_to(Point::new(c.x - sz * 0.16, c.y + sz * 0.22));
        pb.quadratic_curve_to(
            Point::new(c.x, c.y + sz * 0.34),
            Point::new(c.x + sz * 0.16, c.y + sz * 0.22),
        );
    });
    frame.stroke(
        &mouth,
        canvas::Stroke::default()
            .with_color(ink)
            .with_width(2.5 * k)
            .with_line_cap(canvas::LineCap::Round),
    );

    // The name tag, under the body.
    let tag_w = tag_width(&n.label);
    let tag_top = c.y + half + 8.0 * k;
    let tag = canvas::Path::rectangle(
        Point::new(c.x - tag_w / 2.0, tag_top),
        Size::new(tag_w, 22.0),
    );
    frame.fill(&tag, crate::theme::palette().paper.scale_alpha(alpha));
    frame.stroke(
        &tag,
        canvas::Stroke::default().with_color(ink).with_width(1.0),
    );
    frame.fill_text(canvas::Text {
        content: n.label.clone(),
        position: Point::new(c.x - tag_w / 2.0 + LABEL_PAD, tag_top + 4.0),
        color: ink,
        size: LABEL_SIZE.into(),
        font: crate::theme::SANS,
        ..canvas::Text::default()
    });
}

/// A cluster's drawn box in pixels: its world size scaled, but never narrower than its label or its
/// count, which are drawn at a fixed size.
fn cluster_box(n: &Node, k: f32) -> (f32, f32) {
    let count = label_width(&format!("{} agents", n.members.len()));
    (
        (n.w * k).max(label_width(&n.label)).max(count),
        (n.h * k).max(56.0),
    )
}

/// A cluster: a labelled box with its member count, and up to three of its members as dots.
fn draw_cluster(frame: &mut canvas::Frame, n: &Node, c: Point, k: f32, alpha: f32, selected: bool) {
    let (w, h) = cluster_box(n, k);
    let (x0, y0) = (c.x - w / 2.0, c.y - h / 2.0);
    let ink = crate::theme::palette().ink.scale_alpha(alpha);
    let radius = border::Radius::from(8.0 * k);

    frame.fill(
        &canvas::Path::rounded_rectangle(Point::new(x0 + 3.0, y0 + 3.0), Size::new(w, h), radius),
        crate::theme::palette().ink.scale_alpha(0.85 * alpha),
    );
    let body = canvas::Path::rounded_rectangle(Point::new(x0, y0), Size::new(w, h), radius);
    frame.fill(&body, crate::theme::palette().paper.scale_alpha(alpha));
    frame.stroke(
        &body,
        canvas::Stroke::default()
            .with_color(ink)
            .with_width(2.5 * k),
    );
    if selected {
        let ring = canvas::Path::rounded_rectangle(
            Point::new(x0 - 5.0, y0 - 5.0),
            Size::new(w + 10.0, h + 10.0),
            radius,
        );
        frame.stroke(
            &ring,
            canvas::Stroke::default()
                .with_color(crate::theme::palette().signal)
                .with_width(2.0),
        );
    }

    frame.fill_text(canvas::Text {
        content: n.label.clone(),
        position: Point::new(x0 + LABEL_PAD, y0 + 7.0),
        color: ink,
        size: LABEL_SIZE.into(),
        font: crate::theme::SANS,
        ..canvas::Text::default()
    });
    frame.fill_text(canvas::Text {
        content: format!("{} agents", n.members.len()),
        position: Point::new(x0 + LABEL_PAD, y0 + 26.0),
        color: crate::theme::palette().ink_2.scale_alpha(alpha),
        size: 12.0.into(),
        font: crate::theme::SANS,
        ..canvas::Text::default()
    });
    for (i, cid) in n.members.iter().take(3).enumerate() {
        let dot = canvas::Path::circle(Point::new(x0 + 14.0 + i as f32 * 12.0, y0 + h - 12.0), 4.5);
        frame.fill(&dot, ui::hsl(agent_hue(*cid), 0.6, 0.64).scale_alpha(alpha));
        frame.stroke(
            &dot,
            canvas::Stroke::default().with_color(ink).with_width(1.2),
        );
    }
}

// ---- Breadcrumb and controls -----------------------------------------------

/// The controls above the graph: the family toggle, and Back when a level is open.
fn toolbar<'a>(grouped: bool, depth: usize) -> Element<'a, Msg> {
    let rule = ui::frame(None, crate::theme::palette().ink);
    let style = move |t: &Theme| if grouped { on_box(t) } else { rule(t) };
    let colour = if grouped {
        crate::theme::palette().paper
    } else {
        crate::theme::palette().ink
    };
    let toggle = mouse_area(
        container(ui::sans("GROUP BY FAMILY", 11.0, colour))
            .padding([4.0, 8.0])
            .style(style),
    )
    .on_press(Msg::Group);
    let mut bar = row![toggle].spacing(8);
    if grouped && depth > 0 {
        bar = bar.push(ui::btn(
            "Back",
            None,
            Kind::Secondary,
            Some(Msg::Level(depth - 1)),
        ));
    }
    bar.push(Space::new().width(Length::Fill)).into()
}

fn on_box(_: &Theme) -> container::Style {
    filled(crate::theme::palette().ink)
}

/// The breadcrumb, "All families › Build › builder": each crumb but the last one climbs back to it.
fn crumbs<'a>(nav: &[Key]) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![crumb("All families", 0, nav.is_empty())];
    for (i, key) in nav.iter().enumerate() {
        let label = match key {
            Key::Family(f) => family_label(f).to_string(),
            Key::Role(_, r) => r.clone(),
            Key::Agent(_) => String::new(),
        };
        parts.push(ui::faint("›").into());
        parts.push(crumb(&label, i + 1, i + 1 == nav.len()));
    }
    row(parts).spacing(8).into()
}

fn crumb<'a>(label: &str, level: usize, current: bool) -> Element<'a, Msg> {
    let text = ui::sans(
        label.to_string(),
        12.0,
        if current {
            crate::theme::palette().ink
        } else {
            crate::theme::palette().ink_2
        },
    );
    if current {
        text.into()
    } else {
        mouse_area(text).on_press(Msg::Level(level)).into()
    }
}

// ---- Panel -----------------------------------------------------------------

/// The bond from `from` to `to`, if there is one.
fn bond_between<'a>(d: &'a Society, from: u64, to: u64) -> Option<&'a Bond> {
    d.bonds.iter().find(|b| b.from == from && b.to == to)
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
        let mut top = row![
            ui::btn(
                &peer_name,
                None,
                Kind::Ghost,
                Some(Msg::Pick(Some(Key::Agent(peer))))
            ),
            ui::badge(kind, kind_tone)
        ]
        .spacing(8);
        if gap(peer) {
            top = top.push(ui::badge("they disagree", Tone::Warn));
        }
        let mut item = column![top, bar(&bond.opinion)].spacing(6);
        if let Some(note) = bond
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            let open = state.open_notes.contains(&peer);
            let long = note.chars().count() > NOTE_PREVIEW;
            let shown = if long && !open {
                format!("{}…", note.chars().take(NOTE_PREVIEW).collect::<String>())
            } else {
                note.to_string()
            };
            item = item.push(ui::markdown(&shown));
            if long {
                item = item.push(ui::btn(
                    if open { "Show less" } else { "Show all" },
                    None,
                    Kind::Ghost,
                    Some(Msg::Note(peer)),
                ));
            }
        }
        container(item)
            .padding([10.0, 0.0])
            .width(Length::Fill)
            .style(ui::frame(None, crate::theme::palette().rule))
            .into()
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

    let mut title = row![ui::sans(name.clone(), 18.0, crate::theme::palette().ink)]
        .spacing(8)
        .align_y(Alignment::Center);
    if state.suspended.contains(&me.cid) {
        title = title.push(ui::badge("suspended", Tone::Danger));
    }
    let mut panel = column![
        column![title, ui::faint(me.role.clone())].spacing(4),
        stat(
            "Reputation",
            if rated {
                format!("{} / 100", rep.round())
            } else {
                "Unrated".to_string()
            }
        ),
        meter(if rated { rep / 100.0 } else { 0.0 }),
        stat(
            "Confidence",
            format!("{}%", (clamp(me.confidence, 0.0, 1.0) * 100.0).round())
        ),
        stat("Wallet", compact(me.wallet)),
        row![
            ui::btn(
                "Trust more",
                None,
                Kind::Primary,
                Some(Msg::Rate(me.cid, 1.0))
            ),
            ui::btn(
                "Trust less",
                None,
                Kind::Secondary,
                Some(Msg::Rate(me.cid, -1.0))
            ),
        ]
        .spacing(8),
        ui::label(&format!("How {name} sees others")),
    ]
    .spacing(14);
    let outgoing_list: Element<'a, Msg> = if outgoing.is_empty() {
        ui::faint("No contacts yet.").into()
    } else {
        column(outgoing).spacing(4).into()
    };
    let incoming_list: Element<'a, Msg> = if incoming.is_empty() {
        ui::faint("Nobody has met them yet.").into()
    } else {
        column(incoming).spacing(4).into()
    };
    panel = panel.push(outgoing_list);
    panel = panel.push(ui::label(&format!("How others see {name}")));
    panel = panel.push(incoming_list);

    let mut out: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        out.push(ui::error_line(e));
    }
    out.push(panel.into());
    column(out).spacing(14).padding(4.0).into()
}

/// A family's or a role's panel: its members, the trust that runs in and out of it, and its
/// strongest ties outside.
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
    let shown = |v: Option<f32>| {
        v.map(|e| format!("{e:.2}"))
            .unwrap_or_else(|| "none yet".to_string())
    };
    let mut ties = outside.clone();
    ties.sort_by(|a, b| b.strength.total_cmp(&a.strength));

    let title = match key {
        Key::Family(f) => family_label(f).to_string(),
        Key::Role(_, r) => r.clone(),
        Key::Agent(_) => String::new(),
    };
    let member_list: Element<'a, Msg> = if members.is_empty() {
        ui::faint("No agents here.").into()
    } else {
        column(
            members
                .iter()
                .map(|a| ui::btn(&name_of(a), None, Kind::Ghost, Some(Msg::Member(a.cid))))
                .collect::<Vec<Element<'a, Msg>>>(),
        )
        .spacing(4)
        .into()
    };
    let tie_list: Vec<Element<'a, Msg>> = ties
        .iter()
        .take(6)
        .map(|b| -> Element<'a, Msg> {
            let (mine, other) = if inside.contains(&b.from) {
                (b.from, b.to)
            } else {
                (b.to, b.from)
            };
            let mine_name = names
                .get(&mine)
                .map(|a| name_of(a))
                .unwrap_or_else(|| format!("agent {mine}"));
            let other_name = names
                .get(&other)
                .map(|a| name_of(a))
                .unwrap_or_else(|| format!("agent {other}"));
            column![
                ui::btn(&other_name, None, Kind::Ghost, Some(Msg::Member(other))),
                ui::faint(format!("{mine_name} · strength {:.1}", b.strength)),
                bar(&b.opinion),
            ]
            .spacing(6)
            .into()
        })
        .collect();
    let ties_col: Element<'a, Msg> = if tie_list.is_empty() {
        ui::faint("No ties outside yet.").into()
    } else {
        column(tie_list).spacing(10).into()
    };

    let panel = column![
        column![
            ui::sans(title, 18.0, crate::theme::palette().ink),
            ui::faint(format!("{} agents", members.len()))
        ]
        .spacing(4),
        stat("Mean trust in", shown(trust_in)),
        stat("Mean trust out", shown(trust_out)),
        ui::label("Members"),
        member_list,
        ui::label("Strongest outside ties"),
        ties_col,
    ]
    .spacing(14);
    column![panel].padding(4.0).into()
}

fn stat<'a>(key: &'static str, value: String) -> Element<'a, Msg> {
    row![
        ui::faint(key),
        Space::new().width(Length::Fill),
        ui::sans(value, 13.0, crate::theme::palette().ink)
    ]
    .into()
}

/// Reputation as a bar, filled to `frac`.
fn meter<'a>(frac: f32) -> Element<'a, Msg> {
    let filled = permille(frac);
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if filled > 0 {
        parts.push(tint(
            Length::FillPortion(filled),
            crate::theme::palette().ink,
        ));
    }
    if filled < 1000 {
        parts.push(
            Space::new()
                .width(Length::FillPortion(1000 - filled))
                .height(Length::Fill)
                .into(),
        );
    }
    container(row(parts).height(Length::Fill))
        .width(Length::Fill)
        .height(6.0)
        .style(ui::frame(None, crate::theme::palette().ink))
        .into()
}

/// An opinion: the filled part is the expected trust e; the lighter tail runs on by the uncertainty u.
fn bar<'a>(o: &Opinion) -> Element<'a, Msg> {
    let e = clamp(o.e, 0.0, 1.0);
    let tail = clamp(o.u, 0.0, 1.0).min(1.0 - e);
    let fill = permille(e);
    let tail_w = permille(tail).min(1000 - fill);
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if fill > 0 {
        parts.push(tint(Length::FillPortion(fill), band(e)));
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
    let track = container(row(parts).height(Length::Fill))
        .width(Length::Fill)
        .height(8.0)
        .style(ui::frame(None, crate::theme::palette().ink));
    row![
        track,
        ui::sans(format!("{e:.2}"), 12.0, crate::theme::palette().ink)
    ]
    .spacing(8)
    .into()
}

/// The page with nothing selected: how to read the graph.
fn legend<'a>() -> Element<'a, Msg> {
    let line =
        |colour: Color, height: f32| -> Element<'a, Msg> { tint_sized(18.0, height, colour) };
    let dashed = || -> Element<'a, Msg> {
        row![
            tint_sized(5.0, 2.0, crate::theme::palette().ink_3),
            tint_sized(5.0, 2.0, crate::theme::palette().ink_3),
            tint_sized(5.0, 2.0, crate::theme::palette().ink_3)
        ]
        .spacing(2)
        .into()
    };
    let entry = |swatch: Element<'a, Msg>, text: &'static str| -> Element<'a, Msg> {
        row![
            container(swatch).width(24.0).center_y(Length::Shrink),
            ui::prose(text)
        ]
        .spacing(10)
        .into()
    };
    column![
        column![
            ui::sans("Nothing selected", 18.0, crate::theme::palette().ink),
            ui::faint("Pick an agent to see how it sees the others, and how they see it.")
        ]
        .spacing(4),
        ui::label("Reading the graph"),
        entry(tint_sized(18.0, 10.0, RED), "trust below 0.4"),
        entry(tint_sized(18.0, 10.0, AMBER), "trust from 0.4 to 0.6"),
        entry(tint_sized(18.0, 10.0, GREEN), "trust above 0.6"),
        entry(
            line(crate::theme::palette().ink_3, 2.0),
            "solid, with an arrowhead: its own opinions"
        ),
        entry(dashed(), "dashed: the opinions held of it"),
        entry(
            line(crate::theme::palette().ink_3, 4.0),
            "thicker line: a stronger contact"
        ),
        entry(
            line(crate::theme::palette().ink_2, 3.0),
            "darker grey: a mutual pair"
        ),
        ui::faint("Trusting an agent more or less moves the opinions held of it."),
    ]
    .spacing(10)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(cid: u64, reputation: f32, name: &str) -> Agent {
        Agent {
            cid,
            name: name.to_string(),
            role: String::new(),
            tier: String::new(),
            reputation,
            confidence: 0.0,
            wallet: 0.0,
        }
    }

    /// An agent with a role, named after its cid.
    fn member(cid: u64, role: &str) -> Agent {
        Agent {
            role: role.to_string(),
            ..agent(cid, 50.0, &format!("agent {cid}"))
        }
    }

    fn bond(from: u64, to: u64, strength: f32) -> Bond {
        Bond {
            from,
            to,
            kind: "met".into(),
            strength,
            note: None,
            opinion: Opinion { e: 0.5, u: 0.0 },
        }
    }

    fn bond_e(from: u64, to: u64, strength: f32, e: f32) -> Bond {
        Bond {
            opinion: Opinion { e, u: 0.0 },
            ..bond(from, to, strength)
        }
    }

    /// Sixteen agents, ring and cross bonds, names of mixed length.
    fn crowd() -> (Vec<Agent>, Vec<Bond>) {
        let names = [
            "Ada",
            "Marlowe",
            "Alexandria Quillbottom",
            "Vance",
            "Nadia",
            "Ingrid",
            "Mira",
            "Vera",
            "Rowan",
            "Marisol",
            "Alden",
            "Tally",
            "Della",
            "Quill",
            "Marco",
            "Alder",
        ];
        let agents = (0..16)
            .map(|i| agent(i, (i * 37 % 100) as f32, names[i as usize]))
            .collect();
        let bonds = (0..16)
            .flat_map(|i| [bond(i, (i + 1) % 16, 8.0), bond(i, (i + 5) % 16, 3.0)])
            .collect();
        (agents, bonds)
    }

    /// Every node lies inside the box, and no two footprints clash.
    fn assert_clear(nodes: &[Node], pos: &HashMap<Key, Pt>) {
        for n in nodes {
            let (p, f) = (pos[&n.key], n.fp);
            assert!(
                p.x - f.half >= 0.0 && p.x + f.half <= LW,
                "{:?} leaves the box sideways",
                n.key
            );
            assert!(
                p.y - f.up >= 0.0 && p.y + f.down <= LH,
                "{:?} leaves the box vertically",
                n.key
            );
        }
        for (i, a) in nodes.iter().enumerate() {
            for b in &nodes[i + 1..] {
                let (ox, oy) = overlap(pos[&a.key], a.fp, pos[&b.key], b.fp);
                assert!(
                    ox <= 0.0 || oy <= 0.0,
                    "{:?} and {:?} clash: {ox:.1} by {oy:.1}",
                    a.key,
                    b.key
                );
            }
        }
    }

    #[test]
    fn layout_spreads_agents_over_the_box_without_clashes() {
        let (agents, bonds) = crowd();
        let s = build_scene(&Society { agents, bonds }, false, &[]);
        let mut pos = HashMap::new();
        layout(&s.nodes, &s.links, &mut pos);
        assert_clear(&s.nodes, &pos);
        let xs: Vec<f32> = s.nodes.iter().map(|n| pos[&n.key].x).collect();
        let ys: Vec<f32> = s.nodes.iter().map(|n| pos[&n.key].y).collect();
        let span = |v: &[f32]| {
            v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!(
            span(&xs) > 0.6 * LW,
            "the graph is not spread across the width"
        );
        assert!(
            span(&ys) > 0.6 * LH,
            "the graph is not spread across the height"
        );
    }

    #[test]
    fn a_new_agent_joins_without_moving_the_placed_ones() {
        let (agents, bonds) = crowd();
        let mut d = Society { agents, bonds };
        let mut pos = HashMap::new();
        let s = build_scene(&d, false, &[]);
        layout(&s.nodes, &s.links, &mut pos);
        let before = pos.clone();
        d.agents.push(agent(99, 50.0, "Newcomer"));
        let s = build_scene(&d, false, &[]);
        layout(&s.nodes, &s.links, &mut pos);
        for (key, p) in &before {
            assert_eq!(pos[key], *p, "{key:?} moved");
        }
        assert_clear(&s.nodes, &pos);
    }

    #[test]
    fn roles_map_to_families() {
        assert_eq!(role_family("builder", ""), "build");
        assert_eq!(role_family("security-reviewer", ""), "operations");
        assert_eq!(role_family("librarian", ""), "knowledge");
        assert_eq!(role_family("auditor", ""), "money");
        assert_eq!(role_family("mayor", ""), "command");
        assert_eq!(
            role_family("builder", "orchestrator"),
            "command",
            "the orchestrator tier is the command family"
        );
        assert_eq!(
            role_family("tinker", ""),
            "other",
            "an unknown role is other"
        );
        assert_eq!(family_label("money"), "Money");
        assert_eq!(family_label("unknown"), "Other");
    }

    #[test]
    fn cross_contacts_between_two_clusters_make_one_edge() {
        let d = Society {
            agents: vec![
                member(1, "builder"),
                member(2, "architect"),
                member(3, "finance"),
                member(4, "auditor"),
            ],
            bonds: vec![
                bond_e(1, 3, 2.0, 0.2),
                bond_e(2, 4, 3.0, 0.8),
                bond_e(3, 1, 1.0, 0.5),
                bond_e(1, 2, 4.0, 0.9),
            ],
        };
        let key_of = |cid: u64| {
            d.agents
                .iter()
                .find(|a| a.cid == cid)
                .map(|a| Key::Family(family_of(a)))
        };
        let edges = links(&d.bonds, key_of, true);
        assert_eq!(
            edges.len(),
            1,
            "the bond inside Build is left out, the cross bonds make one edge"
        );
        assert!((edges[0].strength - 6.0).abs() < 1e-4, "summed strength");
        assert!(
            (edges[0].e - 0.5).abs() < 1e-4,
            "mean trust of 0.2, 0.8 and 0.5"
        );
    }

    #[test]
    fn families_open_into_roles_and_a_single_role_collapses() {
        let d = Society {
            agents: vec![
                member(1, "builder"),
                member(2, "builder"),
                member(3, "builder"),
                member(4, "architect"),
                member(5, "finance"),
            ],
            bonds: vec![],
        };
        let keys = |nav: &[Key]| -> Vec<Key> {
            build_scene(&d, true, nav)
                .nodes
                .into_iter()
                .map(|n| n.key)
                .collect()
        };
        let top = keys(&[]);
        assert!(top.contains(&Key::Family("build")) && top.contains(&Key::Family("money")));

        let build = keys(&[Key::Family("build")]);
        assert!(
            build.contains(&Key::Role("build", "builder".into())),
            "a role with several agents is a cluster"
        );
        assert!(
            build.contains(&Key::Agent(4)),
            "a role with one agent shows that agent directly"
        );
        assert!(
            !build.contains(&Key::Family("build")),
            "the open family is replaced by its roles"
        );
        assert!(
            build.contains(&Key::Family("money")),
            "the other families stay as clusters"
        );

        let mut nav = vec![Key::Family("build"), Key::Role("build", "builder".into())];
        let role = keys(&nav);
        assert!(
            role.contains(&Key::Agent(1))
                && role.contains(&Key::Agent(2))
                && role.contains(&Key::Agent(3))
        );
        assert!(!role.contains(&Key::Role("build", "builder".into())));

        // The breadcrumb's "Build" climbs back to the family's roles.
        nav.truncate(1);
        assert_eq!(keys(&nav), build);
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
