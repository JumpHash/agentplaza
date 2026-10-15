//! `agentplaza templates`: the hub machine's warm templates. A new agent (or E2B sandbox) of a
//! template's shape starts by restoring it instead of booting; a template is built on first use and
//! rebuilt when the guest image changes. Removing one only makes the next start of that shape build it
//! again.

use clap::Subcommand;

use crate::ctx::{self, Ctx, confirm, seg};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The warm templates: their shape (role, vCPUs, memory), when they were built, and their disk.
    List,
    /// Remove a template (VMs restored from it keep running).
    Rm {
        key: String,
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let v = ctx.client.get("/v1/templates")?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = v
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|t| {
                        vec![
                            t["key"]
                                .as_str()
                                .unwrap_or_default()
                                .chars()
                                .take(12)
                                .collect(),
                            t["role"].as_str().unwrap_or("(sandbox)").to_string(),
                            t["vcpus"].to_string(),
                            t["memory_mib"].to_string(),
                            format!("{} ms", t["build_ms"]),
                            format!("{:.1} MB", t["disk_bytes"].as_f64().unwrap_or(0.0) / 1e6),
                        ]
                    })
                    .collect();
                ctx::table(
                    &["key", "role", "vcpus", "memory MiB", "build", "disk"],
                    &rows,
                );
            });
            Ok(())
        }
        Cmd::Rm { key, yes } => {
            if !confirm(
                yes,
                &format!("Remove template {key}? The next start of its shape builds it again."),
            )? {
                println!("cancelled");
                return Ok(());
            }
            let v = ctx.client.delete(&format!("/v1/templates/{}", seg(&key)))?;
            ctx.out(&v, |_| println!("removed template {key}"));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn templates_parse() {
        let cli = crate::Cli::try_parse_from(["agentplaza", "templates", "rm", "abc", "--yes"])
            .expect("parses");
        assert!(matches!(
            cli.command,
            crate::Command::Templates(super::Cmd::Rm { ref key, yes: true }) if key == "abc"
        ));
    }
}
