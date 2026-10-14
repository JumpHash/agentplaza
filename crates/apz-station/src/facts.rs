//! What a daemon reports about its machine, in `hello` and in `Event::Facts` when it changes.

use serde::{Deserialize, Serialize};

/// The operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

/// What kind of machine it is. A VM reports `vm`, whatever its guest OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form {
    Laptop,
    Desktop,
    Mini,
    Server,
    Vm,
}

/// The OS account the agents work in. `agent_user` is the separate account made for the agents;
/// `owner` means the owner's own account was chosen at setup, and the hub shows an "agents act as
/// <name>" badge wherever the machine appears.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AccountMode {
    AgentUser { name: String },
    Owner { name: String },
}

/// One GPU. `vram_mb` is 0 when the GPU shares system memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub vendor: String,
    pub name: String,
    pub vram_mb: u64,
}

/// One display: its OS id, its size in points, and its backing scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    pub id: u32,
    pub w: u32,
    pub h: u32,
    pub scale: f32,
}

/// The machine's facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    pub os: Os,
    pub os_version: String,
    pub arch: String,
    pub hostname: String,
    pub form: Form,
    pub cpus: u32,
    pub ram_mb: u64,
    pub gpus: Vec<Gpu>,
    pub displays: Vec<Display>,
    pub account: AccountMode,
    /// How many applications the daemon found installed.
    pub apps: u32,
    /// Whether the machine can make virtual displays, so pixel sessions do not take the owner's screen.
    pub virtual_displays: bool,
    /// Whether the OS accessibility tree is available (the UI tree and `UiAct` work).
    pub ui_tree: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_wire;

    #[test]
    fn os_and_form_are_snake_case_words() {
        for (os, json) in [
            (Os::Macos, "macos"),
            (Os::Windows, "windows"),
            (Os::Linux, "linux"),
        ] {
            assert_wire(&os, &format!(r#""{json}""#));
        }
        for (form, json) in [
            (Form::Laptop, "laptop"),
            (Form::Desktop, "desktop"),
            (Form::Mini, "mini"),
            (Form::Server, "server"),
            (Form::Vm, "vm"),
        ] {
            assert_wire(&form, &format!(r#""{json}""#));
        }
    }

    #[test]
    fn account_mode_names_the_account() {
        assert_wire(
            &AccountMode::AgentUser {
                name: "agentws".into(),
            },
            r#"{"mode":"agent_user","name":"agentws"}"#,
        );
        assert_wire(
            &AccountMode::Owner {
                name: "irfan".into(),
            },
            r#"{"mode":"owner","name":"irfan"}"#,
        );
    }

    #[test]
    fn gpus_and_displays_round_trip() {
        assert_wire(
            &Gpu {
                vendor: "nvidia".into(),
                name: "RTX 4090".into(),
                vram_mb: 24576,
            },
            r#"{"vendor":"nvidia","name":"RTX 4090","vram_mb":24576}"#,
        );
        assert_wire(
            &Display {
                id: 2,
                w: 2560,
                h: 1440,
                scale: 1.0,
            },
            r#"{"id":2,"w":2560,"h":1440,"scale":1.0}"#,
        );
    }
}
