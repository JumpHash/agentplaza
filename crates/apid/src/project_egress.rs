//! A project's egress on a Mac node (noded's `mac_sandbox::ProjectProxy`). The node's proxy asks apid as the
//! project's party (`apz_egress_rules::project_party`), so the hub knows the project by the CID the question
//! carries. Its connections are decided by the project's rules: the project must be working (a paused, archived or
//! removed project is held, as its agents are), the laws and the credential rules see the `project` role, and the
//! fetch blocklist applies as it does to every party.

use apz_egress_rules::{PROJECT_PARTY_BASE, project_party};

use crate::state::App;

/// The role a project's connections are decided as: the laws and the credential rules name it.
pub const PROJECT_ROLE: &str = "project";

/// The project a party CID stands for, among the projects the hub holds. None for any other CID, and for a party no
/// project matches.
pub fn project_of(app: &App, cid: u64) -> Option<String> {
    let party = u32::try_from(cid)
        .ok()
        .filter(|party| (party & 0xf000_0000) == PROJECT_PARTY_BASE)?;
    app.projects
        .list()
        .into_iter()
        .map(|project| project.id)
        .find(|id| project_party(id) == party)
}
