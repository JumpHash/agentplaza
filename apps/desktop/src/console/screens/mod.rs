//! The screens, one module each, all with the same shape: `State` (Default), `Msg` (with a
//! `Go(Id)` variant for navigation), `go(&Msg)`, `load`, `update`, `view`. This registry is the only
//! place that knows them all; a screen module touches nothing outside itself.

pub mod agent_side;
pub mod wizard;
pub mod overview;
pub mod agents;
pub mod projects;
pub mod tasks;
pub mod git;
pub mod library;
pub mod approvals;
pub mod floor;
pub mod society;
pub mod conversations;
pub mod board;
pub mod council;
pub mod treasury;
pub mod rules;
pub mod models;
pub mod tools;
pub mod machines;
pub mod nodes;
pub mod agent;

use iced::{Element, Task};

use super::Ctx;

/// A screen, and its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Id {
    Overview,
    Agents,
    Projects,
    Tasks,
    Git,
    Library,
    Approvals,
    Floor,
    Society,
    Conversations,
    Board,
    Council,
    Treasury,
    Rules,
    Models,
    Tools,
    Machines,
    Nodes,
    /// One agent's page.
    Agent(u64),
}

/// The sidebar's entries: (screen, label, section, icon), in order.
pub const NAV: &[(Id, &str, &str, &str)] = &[
    (Id::Overview, "Overview", "Plaza", "overview"),
    (Id::Agents, "Agents", "Plaza", "agents"),
    (Id::Projects, "Projects", "Plaza", "projects"),
    (Id::Tasks, "Tasks", "Plaza", "tasks"),
    (Id::Git, "Git", "Plaza", "git"),
    (Id::Library, "Library", "Plaza", "library"),
    (Id::Approvals, "Approvals", "Plaza", "approval"),
    (Id::Floor, "Floor", "Plaza", "floor"),
    (Id::Society, "Society", "Social", "society"),
    (Id::Conversations, "Conversations", "Social", "conversations"),
    (Id::Board, "Board", "Social", "boardpin"),
    (Id::Council, "Council", "Social", "council"),
    (Id::Treasury, "Treasury", "Social", "treasury"),
    (Id::Rules, "Rules", "Social", "rules"),
    (Id::Models, "Models", "Setup", "models"),
    (Id::Tools, "Tools", "Setup", "wrench"),
    (Id::Machines, "Machines", "Setup", "machine"),
    (Id::Nodes, "Nodes", "Setup", "link"),
];

#[derive(Debug, Default)]
pub struct Screens {
    pub overview: overview::State,
    pub agents: agents::State,
    pub projects: projects::State,
    pub tasks: tasks::State,
    pub git: git::State,
    pub library: library::State,
    pub approvals: approvals::State,
    pub floor: floor::State,
    pub society: society::State,
    pub conversations: conversations::State,
    pub board: board::State,
    pub council: council::State,
    pub treasury: treasury::State,
    pub rules: rules::State,
    pub models: models::State,
    pub tools: tools::State,
    pub machines: machines::State,
    pub nodes: nodes::State,
    pub agent: agent::State,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Overview(overview::Msg),
    Agents(agents::Msg),
    Projects(projects::Msg),
    Tasks(tasks::Msg),
    Git(git::Msg),
    Library(library::Msg),
    Approvals(approvals::Msg),
    Floor(floor::Msg),
    Society(society::Msg),
    Conversations(conversations::Msg),
    Board(board::Msg),
    Council(council::Msg),
    Treasury(treasury::Msg),
    Rules(rules::Msg),
    Models(models::Msg),
    Tools(tools::Msg),
    Machines(machines::Msg),
    Nodes(nodes::Msg),
    Agent(agent::Msg),
}

/// The navigation a screen's message asks for.
pub fn navigation(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Overview(m) => overview::go(m),
        Msg::Agents(m) => agents::go(m),
        Msg::Projects(m) => projects::go(m),
        Msg::Tasks(m) => tasks::go(m),
        Msg::Git(m) => git::go(m),
        Msg::Library(m) => library::go(m),
        Msg::Approvals(m) => approvals::go(m),
        Msg::Floor(m) => floor::go(m),
        Msg::Society(m) => society::go(m),
        Msg::Conversations(m) => conversations::go(m),
        Msg::Board(m) => board::go(m),
        Msg::Council(m) => council::go(m),
        Msg::Treasury(m) => treasury::go(m),
        Msg::Rules(m) => rules::go(m),
        Msg::Models(m) => models::go(m),
        Msg::Tools(m) => tools::go(m),
        Msg::Machines(m) => machines::go(m),
        Msg::Nodes(m) => nodes::go(m),
        Msg::Agent(m) => agent::go(m),
    }
}

/// Opens a screen (sets its parameters) and loads it.
pub fn open(screens: &mut Screens, id: Id, ctx: &Ctx) -> Task<Msg> {
    if let Id::Agent(cid) = id {
        screens.agent = agent::State { cid, ..Default::default() };
    }
    refresh(screens, &id, ctx)
}

/// Reloads the shown screen.
pub fn refresh(screens: &mut Screens, id: &Id, ctx: &Ctx) -> Task<Msg> {
    match id {
        Id::Overview => overview::load(&mut screens.overview, ctx).map(Msg::Overview),
        Id::Agents => agents::load(&mut screens.agents, ctx).map(Msg::Agents),
        Id::Projects => projects::load(&mut screens.projects, ctx).map(Msg::Projects),
        Id::Tasks => tasks::load(&mut screens.tasks, ctx).map(Msg::Tasks),
        Id::Git => git::load(&mut screens.git, ctx).map(Msg::Git),
        Id::Library => library::load(&mut screens.library, ctx).map(Msg::Library),
        Id::Approvals => approvals::load(&mut screens.approvals, ctx).map(Msg::Approvals),
        Id::Floor => floor::load(&mut screens.floor, ctx).map(Msg::Floor),
        Id::Society => society::load(&mut screens.society, ctx).map(Msg::Society),
        Id::Conversations => conversations::load(&mut screens.conversations, ctx).map(Msg::Conversations),
        Id::Board => board::load(&mut screens.board, ctx).map(Msg::Board),
        Id::Council => council::load(&mut screens.council, ctx).map(Msg::Council),
        Id::Treasury => treasury::load(&mut screens.treasury, ctx).map(Msg::Treasury),
        Id::Rules => rules::load(&mut screens.rules, ctx).map(Msg::Rules),
        Id::Models => models::load(&mut screens.models, ctx).map(Msg::Models),
        Id::Tools => tools::load(&mut screens.tools, ctx).map(Msg::Tools),
        Id::Machines => machines::load(&mut screens.machines, ctx).map(Msg::Machines),
        Id::Nodes => nodes::load(&mut screens.nodes, ctx).map(Msg::Nodes),
        Id::Agent(_) => agent::load(&mut screens.agent, ctx).map(Msg::Agent),
    }
}

pub fn update(screens: &mut Screens, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Overview(m) => overview::update(&mut screens.overview, m, ctx).map(Msg::Overview),
        Msg::Agents(m) => agents::update(&mut screens.agents, m, ctx).map(Msg::Agents),
        Msg::Projects(m) => projects::update(&mut screens.projects, m, ctx).map(Msg::Projects),
        Msg::Tasks(m) => tasks::update(&mut screens.tasks, m, ctx).map(Msg::Tasks),
        Msg::Git(m) => git::update(&mut screens.git, m, ctx).map(Msg::Git),
        Msg::Library(m) => library::update(&mut screens.library, m, ctx).map(Msg::Library),
        Msg::Approvals(m) => approvals::update(&mut screens.approvals, m, ctx).map(Msg::Approvals),
        Msg::Floor(m) => floor::update(&mut screens.floor, m, ctx).map(Msg::Floor),
        Msg::Society(m) => society::update(&mut screens.society, m, ctx).map(Msg::Society),
        Msg::Conversations(m) => conversations::update(&mut screens.conversations, m, ctx).map(Msg::Conversations),
        Msg::Board(m) => board::update(&mut screens.board, m, ctx).map(Msg::Board),
        Msg::Council(m) => council::update(&mut screens.council, m, ctx).map(Msg::Council),
        Msg::Treasury(m) => treasury::update(&mut screens.treasury, m, ctx).map(Msg::Treasury),
        Msg::Rules(m) => rules::update(&mut screens.rules, m, ctx).map(Msg::Rules),
        Msg::Models(m) => models::update(&mut screens.models, m, ctx).map(Msg::Models),
        Msg::Tools(m) => tools::update(&mut screens.tools, m, ctx).map(Msg::Tools),
        Msg::Machines(m) => machines::update(&mut screens.machines, m, ctx).map(Msg::Machines),
        Msg::Nodes(m) => nodes::update(&mut screens.nodes, m, ctx).map(Msg::Nodes),
        Msg::Agent(m) => agent::update(&mut screens.agent, m, ctx).map(Msg::Agent),
    }
}

/// The shown screen's own subscriptions: the Floor's animation frames and the hub's events while it is open.
pub fn subscription(_screens: &Screens, id: &Id, ctx: &Ctx) -> iced::Subscription<Msg> {
    match id {
        Id::Floor => iced::Subscription::batch([floor::subscription(), floor::events(ctx)]).map(Msg::Floor),
        _ => iced::Subscription::none(),
    }
}

pub fn view<'a>(screens: &'a Screens, id: &Id, ctx: &'a Ctx) -> Element<'a, Msg> {
    match id {
        Id::Overview => overview::view(&screens.overview, ctx).map(Msg::Overview),
        Id::Agents => agents::view(&screens.agents, ctx).map(Msg::Agents),
        Id::Projects => projects::view(&screens.projects, ctx).map(Msg::Projects),
        Id::Tasks => tasks::view(&screens.tasks, ctx).map(Msg::Tasks),
        Id::Git => git::view(&screens.git, ctx).map(Msg::Git),
        Id::Library => library::view(&screens.library, ctx).map(Msg::Library),
        Id::Approvals => approvals::view(&screens.approvals, ctx).map(Msg::Approvals),
        Id::Floor => floor::view(&screens.floor, ctx).map(Msg::Floor),
        Id::Society => society::view(&screens.society, ctx).map(Msg::Society),
        Id::Conversations => conversations::view(&screens.conversations, ctx).map(Msg::Conversations),
        Id::Board => board::view(&screens.board, ctx).map(Msg::Board),
        Id::Council => council::view(&screens.council, ctx).map(Msg::Council),
        Id::Treasury => treasury::view(&screens.treasury, ctx).map(Msg::Treasury),
        Id::Rules => rules::view(&screens.rules, ctx).map(Msg::Rules),
        Id::Models => models::view(&screens.models, ctx).map(Msg::Models),
        Id::Tools => tools::view(&screens.tools, ctx).map(Msg::Tools),
        Id::Machines => machines::view(&screens.machines, ctx).map(Msg::Machines),
        Id::Nodes => nodes::view(&screens.nodes, ctx).map(Msg::Nodes),
        Id::Agent(_) => agent::view(&screens.agent, ctx).map(Msg::Agent),
    }
}
