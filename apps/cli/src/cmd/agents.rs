//! `agentplaza agents`: the agents are the VMs the work engine's sessions run on (`sessions` lists
//! them with their cids): talk to one, stop, restart, set its model; cancel its work, retire or wipe it,
//! put it to sleep or wake it, checkpoint, roll back and fork its VM, drive its harness and loops, set
//! permissions, and decide the proposals the agents make. The submodules hold the larger areas:
//! `lifecycle`, `harness`, `oversight` and `records`.

pub mod harness;
pub mod lifecycle;
pub mod oversight;
pub mod records;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm, text_arg};

use self::lifecycle::WipeWhat;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Send an agent a message (its chat).
    Tell {
        /// The agent's cid.
        cid: u64,
        /// The message, or "-" to read it from stdin.
        #[arg(long)]
        text: String,
    },
    /// Stop an agent. Asks to confirm unless --yes.
    Stop {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Restart an agent's VM. Asks to confirm unless --yes.
    Restart {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Delete an agent. Asks to confirm unless --yes.
    Delete {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Set an agent's model override: provider/model, or "default" to go back to the role's.
    Model {
        /// The agent's cid.
        cid: u64,
        /// provider/model, or "default".
        model: String,
    },
    /// Stop the agent's running work. The agent stays on the roster.
    Cancel {
        /// The agent's cid.
        cid: u64,
    },
    /// Retire an agent for good: its VM stops and it leaves the roster. Asks to confirm unless --yes.
    Retire {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Wipe an agent's workspace, and as asked its memory, transcript or identity. Asks to confirm
    /// unless --yes.
    Wipe {
        /// The agent's cid.
        cid: u64,
        /// Also clear its memory notes.
        #[arg(long)]
        memory: bool,
        /// Also clear its transcript.
        #[arg(long)]
        transcript: bool,
        /// Also forget its name, contacts and reputation.
        #[arg(long)]
        identity: bool,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Put an agent to sleep: its VM pauses with its memory kept, and wakes where it was.
    Sleep {
        /// The agent's cid.
        cid: u64,
    },
    /// Wake a sleeping agent.
    Wake {
        /// The agent's cid.
        cid: u64,
    },
    /// Checkpoint an agent's whole VM (disk and memory), to roll back to later.
    Checkpoint {
        /// The agent's cid.
        cid: u64,
        /// A label for the checkpoint, to find it by in `agents checkpoints`.
        #[arg(long)]
        label: Option<String>,
    },
    /// An agent's VM checkpoints: name, label, age, kind, memory and disk written, and pause.
    Checkpoints {
        /// The agent's cid.
        cid: u64,
    },
    /// Roll an agent's VM back to a checkpoint. A safety checkpoint of its current state is taken
    /// first. Asks to confirm unless --yes.
    Rollback {
        /// The agent's cid.
        cid: u64,
        /// The checkpoint's name (see `agents checkpoints`).
        checkpoint: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Delete a VM checkpoint. Asks to confirm unless --yes.
    CheckpointDelete {
        /// The agent's cid.
        cid: u64,
        /// The checkpoint's name (see `agents checkpoints`).
        checkpoint: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Fork an agent into copies: a checkpoint of its VM is taken, and each copy starts from it.
    Fork {
        /// The agent's cid.
        cid: u64,
        /// How many copies, 1 to 8.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=8))]
        n: u64,
    },
    /// Move an agent's VM to another node. The hub copies it over and reports the bytes moved and the time taken.
    Move {
        /// The agent's cid.
        cid: u64,
        /// The node to move it to, by its id (see `nodes list`).
        node: String,
    },
    /// An agent's harness limits. Add `set`, `compress`, `extend` or `steer` to change them or its
    /// running work.
    Harness {
        /// The agent's cid.
        cid: u64,
        #[command(subcommand)]
        action: Option<harness::Action>,
    },
    /// The loops (prompts run on an agent every so many minutes). Add `start` or `stop` to change them.
    Loops {
        /// Only the loops of this agent.
        #[arg(long)]
        cid: Option<u64>,
        #[command(subcommand)]
        action: Option<harness::LoopAction>,
    },
    /// An agent's permission mode. Add `set` to change it.
    Permissions {
        /// The agent's cid.
        cid: u64,
        #[command(subcommand)]
        action: Option<oversight::PermissionAction>,
    },
    /// The agent proposals waiting for you. Add `approve` or `reject` to decide one.
    Proposals {
        #[command(subcommand)]
        action: Option<records::ProposalAction>,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Cancel { cid } => lifecycle::cancel(ctx, cid),
        Cmd::Retire { cid, yes } => lifecycle::retire(ctx, cid, yes),
        Cmd::Wipe {
            cid,
            memory,
            transcript,
            identity,
            yes,
        } => lifecycle::wipe(
            ctx,
            cid,
            WipeWhat {
                memory,
                transcript,
                identity,
            },
            yes,
        ),
        Cmd::Sleep { cid } => lifecycle::sleep(ctx, cid),
        Cmd::Wake { cid } => lifecycle::wake(ctx, cid),
        Cmd::Checkpoint { cid, label } => lifecycle::checkpoint(ctx, cid, label.as_deref()),
        Cmd::Checkpoints { cid } => lifecycle::checkpoints(ctx, cid),
        Cmd::Rollback {
            cid,
            checkpoint,
            yes,
        } => lifecycle::rollback(ctx, cid, &checkpoint, yes),
        Cmd::CheckpointDelete {
            cid,
            checkpoint,
            yes,
        } => lifecycle::checkpoint_delete(ctx, cid, &checkpoint, yes),
        Cmd::Fork { cid, n } => lifecycle::fork(ctx, cid, n),
        Cmd::Move { cid, node } => lifecycle::move_to(ctx, cid, &node),
        Cmd::Harness { cid, action } => harness::harness(ctx, cid, action),
        Cmd::Loops { cid, action } => harness::loops(ctx, cid, action),
        Cmd::Permissions { cid, action } => oversight::permissions(ctx, cid, action),
        Cmd::Proposals { action } => records::proposals(ctx, action),
        Cmd::Tell { cid, text } => tell(ctx, cid, &text),
        Cmd::Stop { cid, yes } => {
            if !confirm(yes, &format!("Stop agent {cid}?"))? {
                return cancelled();
            }
            let v = ctx
                .client
                .post(&format!("/v1/agents/{cid}/stop"), &json!({}))?;
            ctx.out(&v, |_| println!("stopped agent {cid}"));
            Ok(())
        }
        Cmd::Restart { cid, yes } => {
            if !confirm(yes, &format!("Restart agent {cid}? Its VM restarts."))? {
                return cancelled();
            }
            let v = ctx
                .client
                .post(&format!("/v1/agents/{cid}/restart"), &json!({}))?;
            ctx.out(&v, |_| println!("restarting agent {cid}"));
            Ok(())
        }
        Cmd::Delete { cid, yes } => {
            if !confirm(yes, &format!("Delete agent {cid}?"))? {
                return cancelled();
            }
            let v = ctx.client.delete(&format!("/v1/agents/{cid}"))?;
            ctx.out(&v, |_| println!("deleted agent {cid}"));
            Ok(())
        }
        Cmd::Model { cid, model } => {
            let value = if model == "default" {
                Value::Null
            } else {
                Value::String(model.clone())
            };
            let v = ctx
                .client
                .patch(&format!("/v1/agents/{cid}"), &json!({ "model": value }))?;
            ctx.out(&v, |_| {
                if model == "default" {
                    println!("agent {cid} uses its default model again");
                } else {
                    println!("agent {cid} now runs {model}");
                }
            });
            Ok(())
        }
    }
}

pub(crate) fn cancelled() -> Result<(), String> {
    println!("cancelled");
    Ok(())
}

/// The items of a hub array (none when the value is not one).
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// A reason, trimmed. Blank is refused: a lift or a rejection needs one on the record.
fn required_reason(reason: &str) -> Result<String, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        Err("a reason is required: say why".into())
    } else {
        Ok(reason.to_string())
    }
}

/// An agent as people name it: `<role> #<cid>`, since agents have no names. `agent #<cid>` when it has no role.
pub(crate) fn label_of(a: &Value) -> String {
    let role = ctx::s(a, "role");
    let cid = ctx::text(a, "cid");
    if role.is_empty() {
        format!("agent #{cid}")
    } else {
        format!("{role} #{cid}")
    }
}

fn tell(ctx: &Ctx, cid: u64, text: &str) -> Result<(), String> {
    let text = text_arg(text)?;
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text }))?;
    ctx.out(&v, |v| {
        println!("sent to agent {cid} (work {})", ctx::text(v, "work"))
    });
    Ok(())
}
