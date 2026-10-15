//! The screens, one module each, all with the same shape: `State` (Default), `Msg` (with a
//! `Go(Id)` variant for navigation), `go(&Msg)`, `load`, `update`, `view`. This registry is the only
//! place that knows them all; a screen module touches nothing outside itself.

pub mod agent;
pub mod agent_side;
pub mod agents;
pub mod approvals;
pub mod board;
pub mod conversations;
pub mod council;
pub mod e2b;
pub mod egress;
pub mod engine_lab;
pub mod floor;
pub mod git;
pub mod ide;
pub mod ide_chat;
pub mod ide_chrome;
pub mod ide_diff;
pub mod ide_editor;
pub mod ide_files;
pub mod ide_model;
pub mod ide_palette;
pub mod ide_panels;
pub mod ide_right;
pub mod ide_sidebar;
pub mod ide_terminal;
pub mod ide_tree;
pub mod lab;
pub mod lab_breeders;
pub mod lab_common;
pub mod lab_datasets;
pub mod lab_hf;
pub mod lab_job_detail;
pub mod lab_jobs;
pub mod lab_model_detail;
pub mod lab_models;
pub mod lab_recipes;
pub mod library;
pub mod machines;
pub mod mcp;
pub mod mcp_hub;
pub mod mcp_wizard;
pub mod memory_graph;
pub mod models;
pub mod nodes;
pub mod overview;
pub mod projects;
pub mod prompt_editor;
pub mod prompts;
pub mod rules;
pub mod sanctions;
pub mod setup;
pub mod skill_editor;
pub mod skills;
pub mod society;
pub mod tasks;
pub mod tools;
pub mod treasury;
pub mod usage;
pub mod vault;
pub mod wizard;
pub mod workstations;
pub mod workstations_files;
pub mod workstations_live;
pub mod workstations_rec;
pub mod workstations_vms;

use iced::{Element, Task};

use super::Ctx;

/// A screen, and its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Id {
    Overview,
    Agents,
    Projects,
    Prompts,
    Skills,
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
    Usage,
    Tools,
    Mcp,
    Vault,
    Machines,
    Workstations,
    Nodes,
    /// The owner's egress rules: the headers the hub adds to an agent's HTTPS requests.
    Egress,
    /// The owner's E2B keys and the sandboxes running on the hub.
    E2b,
    Setup,
    /// One task in full, on the Tasks screen, by its id.
    Task(String),
    /// One repository's page, on the Git screen, by its name.
    Repo(String),
    /// One agent's page.
    Agent(u64),
    /// The Lab: its overview (the section strip's first tab).
    Lab,
    LabJobs,
    /// One job's page, by its id.
    LabJobDetail(String),
    LabRecipes,
    LabDatasets,
    LabModels,
    /// One model's page, by its id.
    LabModelDetail(String),
    LabBreeders,
    /// The engine's labs (the web's hidden `#/robot-lab`, `#/room-lab`, `#/hardware-lab`): not in the sidebar.
    RobotLab,
    RoomLab,
    HardwareLab,
    /// The IDE, not in the sidebar: one workspace by its id (`agent:<cid>` or `repo:<name>@<ref>`), or the picker when none.
    Ide(Option<String>),
}

/// The sidebar's entries: (screen, label, section, icon), in order.
pub const NAV: &[(Id, &str, &str, &str)] = &[
    (Id::Overview, "Overview", "Plaza", "overview"),
    (Id::Agents, "Agents", "Plaza", "agents"),
    (Id::Prompts, "Prompts", "Plaza", "edit"),
    (Id::Skills, "Skills", "Plaza", "wrench"),
    (Id::Projects, "Projects", "Plaza", "projects"),
    (Id::Tasks, "Tasks", "Plaza", "tasks"),
    (Id::Git, "Git", "Plaza", "git"),
    (Id::Library, "Library", "Plaza", "library"),
    (Id::Approvals, "Approvals", "Plaza", "approval"),
    (Id::Floor, "Floor", "Plaza", "floor"),
    (Id::Society, "Society", "Social", "society"),
    (
        Id::Conversations,
        "Conversations",
        "Social",
        "conversations",
    ),
    (Id::Board, "Board", "Social", "boardpin"),
    (Id::Council, "Council", "Social", "council"),
    (Id::Treasury, "Treasury", "Social", "treasury"),
    (Id::Rules, "Rules", "Social", "rules"),
    (Id::Lab, "Lab", "Lab", "brain"),
    (Id::Models, "Models", "Setup", "models"),
    (Id::Usage, "Usage", "Setup", "zap"),
    (Id::Tools, "Tools", "Setup", "wrench"),
    (Id::Mcp, "MCP", "Setup", "terminal"),
    (Id::Vault, "Vault", "Setup", "key"),
    (Id::Machines, "Machines", "Setup", "machine"),
    (Id::Workstations, "Workstations", "Setup", "harness"),
    (Id::Nodes, "Nodes", "Setup", "link"),
    (Id::Egress, "Egress", "Setup", "external"),
    (Id::E2b, "E2B API", "Setup", "square-code"),
    (Id::Setup, "Setup", "Setup", "settings"),
];

#[derive(Debug, Default)]
pub struct Screens {
    pub overview: overview::State,
    pub agents: agents::State,
    pub projects: projects::State,
    pub prompts: prompts::State,
    pub skills: skills::State,
    pub tasks: tasks::State,
    pub git: git::State,
    pub library: library::State,
    pub approvals: approvals::State,
    pub floor: floor::State,
    pub engine_lab: engine_lab::State,
    pub society: society::State,
    pub conversations: conversations::State,
    pub board: board::State,
    pub council: council::State,
    pub treasury: treasury::State,
    pub rules: rules::State,
    pub models: models::State,
    pub usage: usage::State,
    pub tools: tools::State,
    pub mcp: mcp::State,
    pub vault: vault::State,
    pub machines: machines::State,
    pub workstations: workstations::State,
    pub nodes: nodes::State,
    pub egress: egress::State,
    pub e2b: e2b::State,
    pub setup: setup::State,
    pub agent: agent::State,
    pub lab: lab::State,
    pub lab_jobs: lab_jobs::State,
    pub lab_job_detail: lab_job_detail::State,
    pub lab_recipes: lab_recipes::State,
    pub lab_datasets: lab_datasets::State,
    pub lab_models: lab_models::State,
    pub lab_model_detail: lab_model_detail::State,
    pub lab_breeders: lab_breeders::State,
    pub ide: ide::State,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Overview(overview::Msg),
    Agents(agents::Msg),
    Projects(projects::Msg),
    Prompts(prompts::Msg),
    Skills(skills::Msg),
    Tasks(tasks::Msg),
    Git(git::Msg),
    Library(library::Msg),
    Approvals(approvals::Msg),
    Floor(floor::Msg),
    EngineLab(engine_lab::Msg),
    Society(society::Msg),
    Conversations(conversations::Msg),
    Board(board::Msg),
    Council(council::Msg),
    Treasury(treasury::Msg),
    Rules(rules::Msg),
    Models(models::Msg),
    Usage(usage::Msg),
    Tools(tools::Msg),
    Mcp(mcp::Msg),
    Vault(vault::Msg),
    Machines(machines::Msg),
    Workstations(workstations::Msg),
    Nodes(nodes::Msg),
    Egress(egress::Msg),
    E2b(e2b::Msg),
    Setup(setup::Msg),
    Agent(agent::Msg),
    Lab(lab::Msg),
    LabJobs(lab_jobs::Msg),
    LabJobDetail(lab_job_detail::Msg),
    LabRecipes(lab_recipes::Msg),
    LabDatasets(lab_datasets::Msg),
    LabModels(lab_models::Msg),
    LabModelDetail(lab_model_detail::Msg),
    LabBreeders(lab_breeders::Msg),
    Ide(ide::Msg),
}

/// The navigation a screen's message asks for.
pub fn navigation(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Overview(m) => overview::go(m),
        Msg::Agents(m) => agents::go(m),
        Msg::Projects(m) => projects::go(m),
        Msg::Prompts(m) => prompts::go(m),
        Msg::Skills(m) => skills::go(m),
        Msg::Tasks(m) => tasks::go(m),
        Msg::Git(m) => git::go(m),
        Msg::Library(m) => library::go(m),
        Msg::Approvals(m) => approvals::go(m),
        Msg::Floor(m) => floor::go(m),
        Msg::EngineLab(_) => None,
        Msg::Society(m) => society::go(m),
        Msg::Conversations(m) => conversations::go(m),
        Msg::Board(m) => board::go(m),
        Msg::Council(m) => council::go(m),
        Msg::Treasury(m) => treasury::go(m),
        Msg::Rules(m) => rules::go(m),
        Msg::Models(m) => models::go(m),
        Msg::Usage(m) => usage::go(m),
        Msg::Tools(m) => tools::go(m),
        Msg::Mcp(m) => mcp::go(m),
        Msg::Vault(m) => vault::go(m),
        Msg::Machines(m) => machines::go(m),
        Msg::Workstations(m) => workstations::go(m),
        Msg::Nodes(m) => nodes::go(m),
        Msg::Egress(m) => egress::go(m),
        Msg::E2b(m) => e2b::go(m),
        Msg::Setup(m) => setup::go(m),
        Msg::Agent(m) => agent::go(m),
        Msg::Lab(m) => lab::go(m),
        Msg::LabJobs(m) => lab_jobs::go(m),
        Msg::LabJobDetail(m) => lab_job_detail::go(m),
        Msg::LabRecipes(m) => lab_recipes::go(m),
        Msg::LabDatasets(m) => lab_datasets::go(m),
        Msg::LabModels(m) => lab_models::go(m),
        Msg::LabModelDetail(m) => lab_model_detail::go(m),
        Msg::LabBreeders(m) => lab_breeders::go(m),
        Msg::Ide(m) => ide::go(m),
    }
}

/// Opens a screen (sets its parameters) and loads it.
pub fn open(screens: &mut Screens, id: Id, ctx: &Ctx) -> Task<Msg> {
    // Any screen change drops the new token from the MCP page, so it never outlives the tab.
    mcp_hub::leave(&mut screens.mcp.hub);
    // The new E2B key is shown once, so it does not outlive the tab either.
    e2b::leave(&mut screens.e2b);
    if let Id::Agent(cid) = id {
        screens.agent = agent::State {
            cid,
            ..Default::default()
        };
    }
    // A takeover ends when the owner leaves the Workstations screen: its socket closes, and the hub gives the screen back.
    if !matches!(id, Id::Workstations) {
        workstations::leave(&mut screens.workstations);
    }
    match &id {
        Id::LabJobDetail(job) => {
            screens.lab_job_detail = lab_job_detail::State {
                job: job.clone(),
                ..Default::default()
            }
        }
        Id::LabModelDetail(model) => {
            screens.lab_model_detail = lab_model_detail::State {
                model: model.clone(),
                ..Default::default()
            }
        }
        Id::Task(task) => screens.tasks = tasks::opened(task.clone()),
        Id::Ide(Some(workspace)) => screens.ide.preselect = Some(workspace.clone()),
        Id::Repo(repo) => {
            screens.git = git::State {
                repo: repo.clone(),
                tab: git::Tab::Code,
                ..Default::default()
            }
        }
        _ => {}
    }
    refresh(screens, &id, ctx)
}

/// The shown screen changed from `from` to `to`. The console calls this once per change, after the new screen
/// was opened. The IDE is told when it is left and when it is entered: its terminals and its run's polling stop
/// and resume there. Entering replaces any tabs the open made, so the kept ones are made again once.
pub fn changed(screens: &mut Screens, from: &Id, to: &Id, ctx: &Ctx) -> Task<Msg> {
    match (matches!(from, Id::Ide(_)), matches!(to, Id::Ide(_))) {
        (true, false) => {
            ide::left(&mut screens.ide);
            Task::none()
        }
        (false, true) => ide::entered(&mut screens.ide, ctx).map(Msg::Ide),
        _ => Task::none(),
    }
}

/// Reloads the shown screen.
pub fn refresh(screens: &mut Screens, id: &Id, ctx: &Ctx) -> Task<Msg> {
    match id {
        Id::Overview => overview::load(&mut screens.overview, ctx).map(Msg::Overview),
        Id::Agents => agents::load(&mut screens.agents, ctx).map(Msg::Agents),
        Id::Projects => projects::load(&mut screens.projects, ctx).map(Msg::Projects),
        Id::Prompts => prompts::load(&mut screens.prompts, ctx).map(Msg::Prompts),
        Id::Skills => skills::load(&mut screens.skills, ctx).map(Msg::Skills),
        Id::Tasks | Id::Task(_) => tasks::load(&mut screens.tasks, ctx).map(Msg::Tasks),
        Id::Git | Id::Repo(_) => git::load(&mut screens.git, ctx).map(Msg::Git),
        Id::Library => library::load(&mut screens.library, ctx).map(Msg::Library),
        Id::Approvals => approvals::load(&mut screens.approvals, ctx).map(Msg::Approvals),
        Id::Floor => floor::load(&mut screens.floor, ctx).map(Msg::Floor),
        Id::RobotLab => {
            engine_lab::open(&mut screens.engine_lab, engine_lab::Kind::Robot).map(Msg::EngineLab)
        }
        Id::RoomLab => {
            engine_lab::open(&mut screens.engine_lab, engine_lab::Kind::Room).map(Msg::EngineLab)
        }
        Id::HardwareLab => engine_lab::open(&mut screens.engine_lab, engine_lab::Kind::Hardware)
            .map(Msg::EngineLab),
        Id::Society => society::load(&mut screens.society, ctx).map(Msg::Society),
        Id::Conversations => {
            conversations::load(&mut screens.conversations, ctx).map(Msg::Conversations)
        }
        Id::Board => board::load(&mut screens.board, ctx).map(Msg::Board),
        Id::Council => council::load(&mut screens.council, ctx).map(Msg::Council),
        Id::Treasury => treasury::load(&mut screens.treasury, ctx).map(Msg::Treasury),
        Id::Rules => rules::load(&mut screens.rules, ctx).map(Msg::Rules),
        Id::Models => models::load(&mut screens.models, ctx).map(Msg::Models),
        Id::Usage => usage::load(&mut screens.usage, ctx).map(Msg::Usage),
        Id::Tools => tools::load(&mut screens.tools, ctx).map(Msg::Tools),
        Id::Mcp => mcp::load(&mut screens.mcp, ctx).map(Msg::Mcp),
        Id::Vault => vault::load(&mut screens.vault, ctx).map(Msg::Vault),
        Id::Machines => machines::load(&mut screens.machines, ctx).map(Msg::Machines),
        Id::Workstations => {
            workstations::load(&mut screens.workstations, ctx).map(Msg::Workstations)
        }
        Id::Nodes => nodes::load(&mut screens.nodes, ctx).map(Msg::Nodes),
        Id::Egress => egress::load(&mut screens.egress, ctx).map(Msg::Egress),
        Id::E2b => e2b::load(&mut screens.e2b, ctx).map(Msg::E2b),
        Id::Setup => setup::load(&mut screens.setup, ctx).map(Msg::Setup),
        Id::Agent(_) => agent::load(&mut screens.agent, ctx).map(Msg::Agent),
        Id::Lab => lab::load(&mut screens.lab, ctx).map(Msg::Lab),
        Id::LabJobs => lab_jobs::load(&mut screens.lab_jobs, ctx).map(Msg::LabJobs),
        Id::LabJobDetail(_) => {
            lab_job_detail::load(&mut screens.lab_job_detail, ctx).map(Msg::LabJobDetail)
        }
        Id::LabRecipes => lab_recipes::load(&mut screens.lab_recipes, ctx).map(Msg::LabRecipes),
        Id::LabDatasets => lab_datasets::load(&mut screens.lab_datasets, ctx).map(Msg::LabDatasets),
        Id::LabModels => lab_models::load(&mut screens.lab_models, ctx).map(Msg::LabModels),
        Id::LabModelDetail(_) => {
            lab_model_detail::load(&mut screens.lab_model_detail, ctx).map(Msg::LabModelDetail)
        }
        Id::LabBreeders => lab_breeders::load(&mut screens.lab_breeders, ctx).map(Msg::LabBreeders),
        Id::Ide(_) => ide::load(&mut screens.ide, ctx).map(Msg::Ide),
    }
}

pub fn update(screens: &mut Screens, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Overview(m) => overview::update(&mut screens.overview, m, ctx).map(Msg::Overview),
        Msg::Agents(m) => agents::update(&mut screens.agents, m, ctx).map(Msg::Agents),
        Msg::Projects(m) => projects::update(&mut screens.projects, m, ctx).map(Msg::Projects),
        Msg::Prompts(m) => prompts::update(&mut screens.prompts, m, ctx).map(Msg::Prompts),
        Msg::Skills(m) => skills::update(&mut screens.skills, m, ctx).map(Msg::Skills),
        Msg::Tasks(m) => tasks::update(&mut screens.tasks, m, ctx).map(Msg::Tasks),
        Msg::Git(m) => git::update(&mut screens.git, m, ctx).map(Msg::Git),
        Msg::Library(m) => library::update(&mut screens.library, m, ctx).map(Msg::Library),
        Msg::Approvals(m) => approvals::update(&mut screens.approvals, m, ctx).map(Msg::Approvals),
        Msg::Floor(m) => floor::update(&mut screens.floor, m, ctx).map(Msg::Floor),
        Msg::EngineLab(m) => {
            engine_lab::update(&mut screens.engine_lab, m, ctx).map(Msg::EngineLab)
        }
        Msg::Society(m) => society::update(&mut screens.society, m, ctx).map(Msg::Society),
        Msg::Conversations(m) => {
            conversations::update(&mut screens.conversations, m, ctx).map(Msg::Conversations)
        }
        Msg::Board(m) => board::update(&mut screens.board, m, ctx).map(Msg::Board),
        Msg::Council(m) => council::update(&mut screens.council, m, ctx).map(Msg::Council),
        Msg::Treasury(m) => treasury::update(&mut screens.treasury, m, ctx).map(Msg::Treasury),
        Msg::Rules(m) => rules::update(&mut screens.rules, m, ctx).map(Msg::Rules),
        Msg::Models(m) => models::update(&mut screens.models, m, ctx).map(Msg::Models),
        Msg::Usage(m) => usage::update(&mut screens.usage, m, ctx).map(Msg::Usage),
        Msg::Tools(m) => tools::update(&mut screens.tools, m, ctx).map(Msg::Tools),
        Msg::Mcp(m) => mcp::update(&mut screens.mcp, m, ctx).map(Msg::Mcp),
        Msg::Vault(m) => vault::update(&mut screens.vault, m, ctx).map(Msg::Vault),
        Msg::Machines(m) => machines::update(&mut screens.machines, m, ctx).map(Msg::Machines),
        Msg::Workstations(m) => {
            workstations::update(&mut screens.workstations, m, ctx).map(Msg::Workstations)
        }
        Msg::Nodes(m) => nodes::update(&mut screens.nodes, m, ctx).map(Msg::Nodes),
        Msg::Egress(m) => egress::update(&mut screens.egress, m, ctx).map(Msg::Egress),
        Msg::E2b(m) => e2b::update(&mut screens.e2b, m, ctx).map(Msg::E2b),
        Msg::Setup(m) => setup::update(&mut screens.setup, m, ctx).map(Msg::Setup),
        Msg::Agent(m) => agent::update(&mut screens.agent, m, ctx).map(Msg::Agent),
        Msg::Lab(m) => lab::update(&mut screens.lab, m, ctx).map(Msg::Lab),
        Msg::LabJobs(m) => lab_jobs::update(&mut screens.lab_jobs, m, ctx).map(Msg::LabJobs),
        Msg::LabJobDetail(m) => {
            lab_job_detail::update(&mut screens.lab_job_detail, m, ctx).map(Msg::LabJobDetail)
        }
        Msg::LabRecipes(m) => {
            lab_recipes::update(&mut screens.lab_recipes, m, ctx).map(Msg::LabRecipes)
        }
        Msg::LabDatasets(m) => {
            lab_datasets::update(&mut screens.lab_datasets, m, ctx).map(Msg::LabDatasets)
        }
        Msg::LabModels(m) => {
            lab_models::update(&mut screens.lab_models, m, ctx).map(Msg::LabModels)
        }
        Msg::LabModelDetail(m) => {
            lab_model_detail::update(&mut screens.lab_model_detail, m, ctx).map(Msg::LabModelDetail)
        }
        Msg::LabBreeders(m) => {
            lab_breeders::update(&mut screens.lab_breeders, m, ctx).map(Msg::LabBreeders)
        }
        Msg::Ide(m) => ide::update(&mut screens.ide, m, ctx).map(Msg::Ide),
    }
}

/// The shown screen's own subscriptions: the Floor's animation frames and the hub's events while it is open.
pub fn subscription(screens: &Screens, id: &Id, ctx: &Ctx) -> iced::Subscription<Msg> {
    match id {
        Id::Overview => overview::subscription(ctx).map(Msg::Overview),
        Id::Agents => agents::subscription(ctx).map(Msg::Agents),
        Id::Prompts => prompts::subscription(ctx).map(Msg::Prompts),
        Id::Skills => skills::subscription(ctx).map(Msg::Skills),
        Id::Floor => {
            iced::Subscription::batch([floor::subscription(&screens.floor), floor::events(ctx)])
                .map(Msg::Floor)
        }
        Id::RobotLab | Id::RoomLab | Id::HardwareLab => {
            engine_lab::subscription(&screens.engine_lab).map(Msg::EngineLab)
        }
        Id::Machines => machines::subscription(&screens.machines).map(Msg::Machines),
        Id::Workstations => {
            workstations::subscription(&screens.workstations, ctx).map(Msg::Workstations)
        }
        Id::Usage => usage::subscription(&screens.usage).map(Msg::Usage),
        Id::E2b => e2b::subscription().map(Msg::E2b),
        Id::Agent(_) => agent::stream(ctx).map(Msg::Agent),
        Id::Treasury => treasury::subscription(&screens.treasury, ctx).map(Msg::Treasury),
        Id::LabJobs => lab_jobs::subscription(&screens.lab_jobs, ctx).map(Msg::LabJobs),
        Id::Society => society::subscription().map(Msg::Society),
        Id::Lab => lab::subscription(&screens.lab, ctx).map(Msg::Lab),
        Id::LabModels => lab_models::subscription(ctx).map(Msg::LabModels),
        Id::Rules => rules::subscription(ctx).map(Msg::Rules),
        Id::Projects => projects::subscription(ctx).map(Msg::Projects),
        Id::Library => library::subscription(ctx).map(Msg::Library),
        Id::Tasks | Id::Task(_) => tasks::subscription(ctx).map(Msg::Tasks),
        Id::Approvals => approvals::subscription(ctx).map(Msg::Approvals),
        Id::Conversations => conversations::subscription(ctx).map(Msg::Conversations),
        Id::Board => board::subscription(ctx).map(Msg::Board),
        Id::Council => council::subscription(ctx).map(Msg::Council),
        Id::LabDatasets => lab_datasets::subscription(ctx).map(Msg::LabDatasets),
        Id::LabRecipes => lab_recipes::subscription(ctx).map(Msg::LabRecipes),
        Id::Ide(_) => ide::subscription(&screens.ide, ctx).map(Msg::Ide),
        Id::LabJobDetail(_) => lab_common::live(&ctx.client)
            .filter_map(|e| lab_common::training_frame(&e))
            .map(lab_job_detail::Msg::Live)
            .map(Msg::LabJobDetail),
        _ => iced::Subscription::none(),
    }
}

pub fn view<'a>(
    screens: &'a Screens,
    id: &Id,
    ctx: &'a Ctx,
    plaza: ide::Plaza,
) -> Element<'a, Msg> {
    match id {
        Id::Overview => overview::view(&screens.overview, ctx).map(Msg::Overview),
        Id::Agents => agents::view(&screens.agents, ctx).map(Msg::Agents),
        Id::Projects => projects::view(&screens.projects, ctx).map(Msg::Projects),
        Id::Prompts => prompts::view(&screens.prompts, ctx).map(Msg::Prompts),
        Id::Skills => skills::view(&screens.skills, ctx).map(Msg::Skills),
        Id::Tasks | Id::Task(_) => tasks::view(&screens.tasks, ctx).map(Msg::Tasks),
        Id::Git | Id::Repo(_) => git::view(&screens.git, ctx).map(Msg::Git),
        Id::Library => library::view(&screens.library, ctx).map(Msg::Library),
        Id::Approvals => approvals::view(&screens.approvals, ctx).map(Msg::Approvals),
        Id::Floor => floor::view(&screens.floor, ctx).map(Msg::Floor),
        Id::RobotLab | Id::RoomLab | Id::HardwareLab => {
            engine_lab::view(&screens.engine_lab, ctx).map(Msg::EngineLab)
        }
        Id::Society => society::view(&screens.society, ctx).map(Msg::Society),
        Id::Conversations => {
            conversations::view(&screens.conversations, ctx).map(Msg::Conversations)
        }
        Id::Board => board::view(&screens.board, ctx).map(Msg::Board),
        Id::Council => council::view(&screens.council, ctx).map(Msg::Council),
        Id::Treasury => treasury::view(&screens.treasury, ctx).map(Msg::Treasury),
        Id::Rules => rules::view(&screens.rules, ctx).map(Msg::Rules),
        Id::Models => models::view(&screens.models, ctx).map(Msg::Models),
        Id::Usage => usage::view(&screens.usage, ctx).map(Msg::Usage),
        Id::Tools => tools::view(&screens.tools, ctx).map(Msg::Tools),
        Id::Mcp => mcp::view(&screens.mcp, ctx).map(Msg::Mcp),
        Id::Vault => vault::view(&screens.vault, ctx).map(Msg::Vault),
        Id::Machines => machines::view(&screens.machines, ctx).map(Msg::Machines),
        Id::Workstations => workstations::view(&screens.workstations).map(Msg::Workstations),
        Id::Nodes => nodes::view(&screens.nodes, ctx).map(Msg::Nodes),
        Id::Egress => egress::view(&screens.egress).map(Msg::Egress),
        Id::E2b => e2b::view(&screens.e2b, ctx.client.base()).map(Msg::E2b),
        Id::Setup => setup::view(&screens.setup, ctx).map(Msg::Setup),
        Id::Agent(_) => agent::view(&screens.agent, ctx).map(Msg::Agent),
        Id::Lab => lab::view(&screens.lab, ctx).map(Msg::Lab),
        Id::LabJobs => lab_jobs::view(&screens.lab_jobs, ctx).map(Msg::LabJobs),
        Id::LabJobDetail(_) => {
            lab_job_detail::view(&screens.lab_job_detail, ctx).map(Msg::LabJobDetail)
        }
        Id::LabRecipes => lab_recipes::view(&screens.lab_recipes, ctx).map(Msg::LabRecipes),
        Id::LabDatasets => lab_datasets::view(&screens.lab_datasets, ctx).map(Msg::LabDatasets),
        Id::LabModels => lab_models::view(&screens.lab_models, ctx).map(Msg::LabModels),
        Id::LabModelDetail(_) => {
            lab_model_detail::view(&screens.lab_model_detail, ctx).map(Msg::LabModelDetail)
        }
        Id::LabBreeders => lab_breeders::view(&screens.lab_breeders, ctx).map(Msg::LabBreeders),
        Id::Ide(_) => ide::view(&screens.ide, plaza).map(Msg::Ide),
    }
}
