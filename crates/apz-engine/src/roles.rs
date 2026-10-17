//! The role vocabulary the engine shares with the desktop: a role's family (the Floor's grouping)
//! and its label (the Floor's plaques). Moved here from `screens/society.rs` and `screens/board.rs`,
//! which re-export them.

/// The family a role belongs to. The orchestrator tier is the command family, and a role that no
/// family lists is "other".
pub fn role_family(role: &str, tier: &str) -> &'static str {
    if tier.trim().eq_ignore_ascii_case("orchestrator") {
        return "command";
    }
    match role.trim().to_ascii_lowercase().as_str() {
        "orchestrator" | "mayor" | "mayor-office" => "command",
        "architect" | "builder" | "qa" | "reviewer" | "release" => "build",
        "edge-operator" | "incident" | "security-reviewer" => "operations",
        "librarian" | "researcher" | "planner" => "knowledge",
        "finance" | "auditor" => "money",
        _ => "other",
    }
}

/// A role's human label (the web's `roleLabel`); an unknown role shows as its raw string.
pub fn role_label(role: &str) -> String {
    let label = match role {
        "orchestrator" => "Orchestrator",
        "architect" => "Architect",
        "auditor" => "Auditor",
        "breeder" => "Breeder",
        "builder" => "Builder",
        "edge-operator" => "Edge operator",
        "finance" => "Finance",
        "incident" => "Incident",
        "legal-agents-counsel" => "Legal · agents' counsel",
        "legal-agents-defender" => "Legal · agents' defender",
        "legal-agents-ombudsman" => "Legal · agents' ombudsman",
        "legal-gov-judge" => "Legal · government judge",
        "legal-gov-prosecutor" => "Legal · government prosecutor",
        "librarian" => "Librarian",
        "mayor" => "Mayor",
        "mayor-office" => "Mayor · office",
        "planner" => "Planner",
        "qa" => "QA",
        "release" => "Release",
        "researcher" => "Researcher",
        "reviewer" => "Reviewer",
        "security-reviewer" => "Security reviewer",
        other => return other.to_string(),
    };
    label.to_string()
}

/// The hues the roles take, by their name's hash (degrees).
const HUES: [f32; 10] = [
    8.0, 28.0, 46.0, 88.0, 140.0, 172.0, 198.0, 222.0, 262.0, 340.0,
];

/// An agent's own hue (degrees), spread by the golden angle so neighbouring cids differ.
pub fn agent_hue(cid: u64) -> f32 {
    ((cid as f64 * 137.508 + 20.0) % 360.0) as f32
}

/// A role's hue (degrees): one of [`HUES`], by the role name's hash (the web's `roleHue`).
pub fn role_hue(role: &str) -> f32 {
    let mut hash: u32 = 0;
    let mut buf = [0u16; 2];
    for ch in role.chars() {
        let code = ch.encode_utf16(&mut buf)[0] as u32;
        hash = hash.wrapping_mul(31).wrapping_add(code);
    }
    HUES[(hash % HUES.len() as u32) as usize]
}
