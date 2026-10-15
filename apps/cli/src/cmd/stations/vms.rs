//! `agentplaza stations vms`: the workstation VMs the plaza makes on its nodes (docs/design/workstations.md, section 10;
//! the hub's side is `crates/apid/src/stations/vms.rs`). Routes: `GET /v1/stations/vms` (list), `POST /v1/stations/vms`
//! (create), `DELETE /v1/stations/vms/{id}` (delete), `POST /v1/stations/vms/{id}/sleep` and
//! `POST /v1/stations/vms/{id}/wake`. `create` makes one on a node and prints the station it will become; `delete`
//! removes it, and the station it enrolled as is revoked; `sleep` and `wake` act on a plaza VM only (a QEMU VM answers 409).

use clap::{Subcommand, ValueEnum};
use serde_json::{Value, json};

use super::{dash, items, yes_no};
use crate::ctx::{self, Ctx, confirm, seg};

#[derive(Subcommand, Debug)]
pub enum VmsCmd {
    /// The workstation VMs: id, name, node, OS, size, GPU, backend, the station it enrolled as, and whether it is online.
    List,
    /// Make a workstation VM on a node. It enrols as a workstation with its join code; the station it becomes is listed
    /// once it does.
    Create(CreateArgs),
    /// Remove a workstation VM: its VM goes from the node and the station it enrolled as is revoked. Asks first on a
    /// terminal unless --yes.
    Delete {
        /// The VM's id (from `agentplaza stations vms list`).
        id: String,
        /// Remove without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Put a plaza workstation VM to sleep: its memory is kept and it wakes in a fraction of a second.
    Sleep {
        /// The VM's id (from `agentplaza stations vms list`).
        id: String,
    },
    /// Wake a sleeping workstation VM from its memory.
    Wake {
        /// The VM's id (from `agentplaza stations vms list`).
        id: String,
    },
}

/// The `create` flags. The hub checks the sizes, the image and the GPU's shape, and says which one is wrong.
#[derive(clap::Args, Debug, Clone, PartialEq, Eq)]
pub struct CreateArgs {
    /// The VM's name: a DNS label (a-z, 0-9 and '-', up to 63 characters).
    pub name: String,
    /// The node that runs the VM (its id, from `agentplaza nodes`).
    #[arg(long)]
    pub node: String,
    /// The guest OS. macOS runs only on Apple hardware; Windows needs --image.
    #[arg(long, value_enum, default_value_t = OsArg::Linux)]
    pub os: OsArg,
    /// Virtual CPUs, from 1 to 16.
    #[arg(long)]
    pub cpus: u32,
    /// Memory in MiB (at least 2048; 4096 for Windows).
    #[arg(long, value_name = "MIB")]
    pub ram_mb: u32,
    /// Disk in GiB, from 8 to 2048.
    #[arg(long, value_name = "GIB")]
    pub disk_gb: u32,
    /// The GPU: none (the default), virtual, or vfio:<pci-address> (for example vfio:0000:01:00.0).
    #[arg(long)]
    pub gpu: Option<String>,
    /// Windows: the path of an owner-provided disk image on the node. It is copied for the VM, never downloaded.
    #[arg(long, value_name = "PATH")]
    pub image: Option<String>,
    /// The launch backend: auto (QEMU for Windows or a vfio GPU, plaza otherwise), plaza, or qemu.
    #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
    pub backend: BackendArg,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsArg {
    Linux,
    Windows,
    Macos,
}

impl OsArg {
    fn word(self) -> &'static str {
        match self {
            OsArg::Linux => "linux",
            OsArg::Windows => "windows",
            OsArg::Macos => "macos",
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendArg {
    Auto,
    Plaza,
    Qemu,
}

impl BackendArg {
    fn word(self) -> &'static str {
        match self {
            BackendArg::Auto => "auto",
            BackendArg::Plaza => "plaza",
            BackendArg::Qemu => "qemu",
        }
    }
}

const LIST_HEAD: [&str; 11] = [
    "id", "name", "node", "os", "cpus", "ram MiB", "disk GiB", "gpu", "backend", "station",
    "online",
];

/// The `POST /v1/stations/vms` body: the flags as the hub reads them. `gpu` and `image` are sent only when given.
pub fn create_body(args: &CreateArgs) -> Value {
    let mut body = json!({
        "node": args.node,
        "os": args.os.word(),
        "name": args.name,
        "cpus": args.cpus,
        "ram_mb": args.ram_mb,
        "disk_gb": args.disk_gb,
        "backend": args.backend.word(),
    });
    if let Some(gpu) = &args.gpu {
        body["gpu"] = json!(gpu);
    }
    if let Some(image) = &args.image {
        body["image"] = json!(image);
    }
    body
}

pub fn run(ctx: &Ctx, action: Option<VmsCmd>) -> Result<(), String> {
    match action.unwrap_or(VmsCmd::List) {
        VmsCmd::List => {
            let vms = ctx.client.get("/v1/stations/vms")?;
            ctx.out(&vms, |v| {
                let rows: Vec<Vec<String>> = items(v).iter().map(vm_row).collect();
                ctx::table(&LIST_HEAD, &rows);
            });
        }
        VmsCmd::Create(args) => {
            let made = ctx.client.post("/v1/stations/vms", &create_body(&args))?;
            ctx.out(&made, |v| {
                println!(
                    "workstation VM {} ({}) on {}: {}, {} vCPUs, {} MiB, {} GiB, gpu {}",
                    ctx::text(v, "id"),
                    ctx::text(v, "name"),
                    ctx::text(v, "node"),
                    ctx::text(v, "os"),
                    ctx::text(v, "cpus"),
                    ctx::text(v, "ram_mb"),
                    ctx::text(v, "disk_gb"),
                    ctx::text(v, "gpu"),
                );
                println!("backend: {}", ctx::text(v, "backend"));
                println!(
                    "it becomes a station when it enrols with join code {}; agentplaza stations list shows it then",
                    ctx::text(v, "code")
                );
            });
        }
        VmsCmd::Delete { id, yes } => {
            if !confirm(
                yes,
                &format!(
                    "Remove workstation VM {id}? Its VM goes from its node, and the station it enrolled as is revoked."
                ),
            )? {
                println!("not removed");
                return Ok(());
            }
            let removed = ctx
                .client
                .delete(&format!("/v1/stations/vms/{}", seg(&id)))?;
            ctx.out(&removed, |v| {
                println!(
                    "workstation VM {} ({}) removed",
                    ctx::text(v, "id"),
                    ctx::text(v, "name")
                )
            });
        }
        VmsCmd::Sleep { id } => power(ctx, &id, "sleep")?,
        VmsCmd::Wake { id } => power(ctx, &id, "wake")?,
    }
    Ok(())
}

/// `POST /v1/stations/vms/{id}/sleep` or `.../wake`: the answer is the VM's view, printed as `<id>: <state>`.
fn power(ctx: &Ctx, id: &str, action: &str) -> Result<(), String> {
    let view = ctx.client.post(
        &format!("/v1/stations/vms/{}/{action}", seg(id)),
        &json!({}),
    )?;
    ctx.out(&view, |v| {
        println!("{}: {}", ctx::text(v, "id"), ctx::text(v, "state"));
    });
    Ok(())
}

/// One VM's row in the list.
fn vm_row(v: &Value) -> Vec<String> {
    vec![
        ctx::text(v, "id"),
        ctx::text(v, "name"),
        ctx::text(v, "node"),
        ctx::text(v, "os"),
        ctx::text(v, "cpus"),
        ctx::text(v, "ram_mb"),
        ctx::text(v, "disk_gb"),
        dash(ctx::text(v, "gpu")),
        dash(ctx::text(v, "backend")),
        dash(ctx::text(v, "station")),
        yes_no(&v["online"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(os: OsArg) -> CreateArgs {
        CreateArgs {
            name: "desk-1".into(),
            node: "n1".into(),
            os,
            cpus: 4,
            ram_mb: 8192,
            disk_gb: 40,
            gpu: None,
            image: None,
            backend: BackendArg::Auto,
        }
    }

    #[test]
    fn create_body_carries_only_what_was_given_for_each_os() {
        // A Linux body names only what was given.
        {
            assert_eq!(
                create_body(&args(OsArg::Linux)),
                json!({"node": "n1", "os": "linux", "name": "desk-1", "cpus": 4, "ram_mb": 8192, "disk_gb": 40, "backend": "auto"})
            );
        }
        // A Windows body carries its image, GPU and backend.
        {
            let mut windows = args(OsArg::Windows);
            windows.image = Some(r"D:\images\win.img".into());
            windows.gpu = Some("vfio:0000:01:00.0".into());
            windows.backend = BackendArg::Qemu;
            assert_eq!(
                create_body(&windows),
                json!({
                    "node": "n1", "os": "windows", "name": "desk-1", "cpus": 4, "ram_mb": 8192, "disk_gb": 40,
                    "backend": "qemu", "gpu": "vfio:0000:01:00.0", "image": r"D:\images\win.img"
                })
            );
        }
        // A macOS body says macos.
        {
            assert_eq!(create_body(&args(OsArg::Macos))["os"], "macos");
        }
    }

    #[test]
    fn a_vm_row_reads_the_view_the_hub_sends() {
        let view = json!({
            "id": "v3", "name": "desk-1", "node": "n1", "os": "linux", "cpus": 4, "ram_mb": 8192,
            "disk_gb": 40, "gpu": "none", "backend": "plaza", "station": null, "online": false
        });
        assert_eq!(
            vm_row(&view),
            vec![
                "v3", "desk-1", "n1", "linux", "4", "8192", "40", "none", "plaza", "-", "no"
            ]
        );
    }
}
