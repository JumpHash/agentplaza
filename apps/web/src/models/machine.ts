/* The node as it reports itself: GET /api/v0/machine.
   Mirrors crates/noded MachineView — never hand-edit without the backend contract. */
export interface Machine {
  /** Host name of the node. */
  name: string;
  /** CPU architecture, e.g. "x86_64" | "aarch64". */
  arch: string;
  /** Hypervisor backend, e.g. "kvm" | "hvf". */
  backend: string;
  /** Interrupt chip, e.g. "ioapic" | "gicv3" | "vgicv2". */
  irqchip: string;
  /** Schema machine class, e.g. "server" | "pi4" | "pi5" | "apple". */
  class: string;
  /** Host CPUs. */
  cpus: number;
  /** Host memory, MiB. */
  memory_mib: number;
  /** How guests get vsock, e.g. "uds-muxer" | "vhost" | "none". */
  vsock_mode: string;
  /** When the daemon started, ms since the Unix epoch. */
  started_at_ms: number;
}
