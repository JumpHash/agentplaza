//! `agentplaza volumes`: the hub's shared read-only data volumes (ext4 images). Each role lists the
//! volumes its agents mount when they are spawned (`role_volumes` in the settings); the guest sees
//! them read-only. Creating one clones an ext4 image or packs a directory into a new image, both on
//! the hub machine. A volume a running VM mounts cannot be removed.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm, seg};

/// The most volumes one role mounts.
const MAX_PER_ROLE: usize = 4;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The shared volumes: their size, the VMs mounting each one, and their source.
    Ls,
    /// Create a volume. SOURCE is a path on the hub machine (an ext4 image is cloned, a directory is
    /// packed into a new ext4 image). It is made absolute here before it is sent.
    Create {
        /// The volume's name.
        name: String,
        /// A path on the hub machine.
        source: String,
    },
    /// Remove a volume. The hub refuses while a VM mounts it.
    Rm {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Each role and the volumes its agents mount. Changes apply to agents spawned from now on.
    Roles,
    /// Set the volumes a role's agents mount, replacing its list (at most 4). Applies to agents
    /// spawned from now on.
    Mount {
        /// The role, e.g. worker.
        role: String,
        /// The volume names, at most 4.
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Remove a role's volume list, so its agents mount none. Applies to agents spawned from now on.
    Unmount {
        /// The role, e.g. worker.
        role: String,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Ls => {
            let v = ctx.client.get("/v1/volumes")?;
            ctx.out(&v, |v| {
                ctx::table(
                    &["NAME", "SIZE (MiB)", "MOUNTED BY", "SOURCE"],
                    &volume_rows(v),
                );
            });
            Ok(())
        }
        Cmd::Create { name, source } => {
            let source = std::fs::canonicalize(&source)
                .map_err(|e| format!("SOURCE {source}: {e}"))?
                .to_string_lossy()
                .into_owned();
            let body = json!({ "name": name, "source": source });
            let v = ctx.client.post("/v1/volumes", &body)?;
            ctx.out(&v, |_| println!("created volume {name}"));
            Ok(())
        }
        Cmd::Rm { name, yes } => {
            if !confirm(
                yes,
                &format!("Remove volume {name}? A VM that mounts it blocks the removal."),
            )? {
                println!("cancelled");
                return Ok(());
            }
            let v = ctx.client.delete(&format!("/v1/volumes/{}", seg(&name)))?;
            ctx.out(&v, |_| println!("removed volume {name}"));
            Ok(())
        }
        Cmd::Roles => {
            let settings = ctx.client.get("/v1/settings")?;
            ctx.out(&settings, |s| {
                ctx::table(&["ROLE", "VOLUMES"], &role_rows(s))
            });
            Ok(())
        }
        Cmd::Mount { role, names } => {
            if names.len() > MAX_PER_ROLE {
                return Err(format!(
                    "a role mounts at most {MAX_PER_ROLE} volumes, not {}",
                    names.len()
                ));
            }
            let mut settings = ctx.client.get("/v1/settings")?;
            set_role(&mut settings, &role, Some(names.as_slice()))?;
            let saved = ctx.client.put("/v1/settings", &settings)?;
            ctx.out(&saved, |_| {
                println!("role {role} mounts {} from now on", names.join(", "))
            });
            Ok(())
        }
        Cmd::Unmount { role } => {
            let mut settings = ctx.client.get("/v1/settings")?;
            set_role(&mut settings, &role, None)?;
            let saved = ctx.client.put("/v1/settings", &settings)?;
            ctx.out(&saved, |_| {
                println!("role {role} mounts no volumes from now on")
            });
            Ok(())
        }
    }
}

/// Sets one role's volume list in the settings object, or removes the role's entry with `None`.
fn set_role(settings: &mut Value, role: &str, names: Option<&[String]>) -> Result<(), String> {
    let root = settings
        .as_object_mut()
        .ok_or("the hub's settings are not an object")?;
    let roles = root
        .entry("role_volumes")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("the hub's role_volumes is not an object")?;
    match names {
        Some(names) => {
            roles.insert(role.to_string(), json!(names));
        }
        None => {
            roles.remove(role);
        }
    }
    Ok(())
}

/// One row per volume: name, size in MiB, the CIDs mounting it, and its source.
fn volume_rows(list: &Value) -> Vec<Vec<String>> {
    list.as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|v| {
            vec![
                ctx::s(v, "name").to_string(),
                format!(
                    "{:.1}",
                    v["size_bytes"].as_f64().unwrap_or(0.0) / 1_048_576.0
                ),
                list_text(&v["in_use"]),
                ctx::s(v, "source").to_string(),
            ]
        })
        .collect()
}

/// One row per role that has volumes: the role and its volume names.
fn role_rows(settings: &Value) -> Vec<Vec<String>> {
    settings["role_volumes"]
        .as_object()
        .map(|roles| {
            roles
                .iter()
                .map(|(role, names)| vec![role.clone(), list_text(names)])
                .collect()
        })
        .unwrap_or_default()
}

/// A JSON array as "a, b", or "-" when it is empty or missing.
fn list_text(v: &Value) -> String {
    let items: Vec<String> = v
        .as_array()
        .map(|a| a.iter().map(plain).collect())
        .unwrap_or_default();
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(", ")
    }
}

/// A JSON string as its text; any other value as its JSON form (a CID stays a number).
fn plain(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::{Value, json};

    use super::{Cmd, role_rows, set_role, volume_rows};

    #[test]
    fn volumes_parse() {
        let cli = crate::Cli::try_parse_from([
            "agentplaza",
            "volumes",
            "mount",
            "worker",
            "data",
            "tools",
        ])
        .expect("parses");
        assert!(matches!(
            cli.command,
            crate::Command::Volumes(Cmd::Mount { ref role, ref names })
                if role == "worker" && names.len() == 2
        ));
    }

    #[test]
    fn mount_needs_at_least_one_name() {
        assert!(crate::Cli::try_parse_from(["agentplaza", "volumes", "mount", "worker"]).is_err());
    }

    #[test]
    fn the_table_shows_size_mounts_and_source() {
        let list = json!([
            {"name": "data", "size_bytes": 2097152, "created_ms": 1, "source": "/srv/data.ext4", "in_use": [3, 7]},
            {"name": "tools", "size_bytes": 1048576, "created_ms": 2, "source": "/srv/tools", "in_use": []}
        ]);
        let rows = volume_rows(&list);
        assert_eq!(rows[0], vec!["data", "2.0", "3, 7", "/srv/data.ext4"]);
        assert_eq!(rows[1], vec!["tools", "1.0", "-", "/srv/tools"]);
    }

    #[test]
    fn roles_lists_each_role_with_its_volumes() {
        let settings = json!({"role_volumes": {"worker": ["data", "tools"], "reviewer": []}});
        let rows = role_rows(&settings);
        assert!(rows.contains(&vec!["worker".to_string(), "data, tools".to_string()]));
        assert!(rows.contains(&vec!["reviewer".to_string(), "-".to_string()]));
    }

    #[test]
    fn mount_sets_a_role_list_and_unmount_removes_it() {
        let mut settings = json!({"max_turns": 5});
        let names = vec!["data".to_string()];
        set_role(&mut settings, "worker", Some(names.as_slice())).expect("sets");
        assert_eq!(settings["role_volumes"]["worker"], json!(["data"]));
        assert_eq!(settings["max_turns"], json!(5));
        set_role(&mut settings, "worker", None).expect("removes");
        assert_eq!(settings["role_volumes"]["worker"], Value::Null);
    }
}
