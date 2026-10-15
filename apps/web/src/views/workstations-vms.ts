// The workstation VMs on the Workstations page (docs/design/workstations.md, section 10): the list of the VMs the plaza
// made on its nodes (state, node, backend, OS, size, GPU, and Delete with a confirm), and the New VM form that makes one.
// The hub checks the form again; the rules and the body live in workstations-vm-model.ts.

import { api, type NodeView, type StationVm, type VmOs } from '../api';
import { href } from '../router';
import { attempt, badge, button, card, empty, input, modal, select, table, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import { type Section } from './workstations-model';
import {
  VM_CPUS,
  VM_DISK_GB,
  VM_FORM_DEFAULTS,
  VM_NAME_MAX,
  VM_RAM_MB,
  VM_WINDOWS_RAM_MB_MIN,
  backendHint,
  backendReason,
  macosOn,
  nodeChoices,
  nodeFactsLine,
  nodeOptionLabel,
  readVmForm,
  showsImage,
  showsPci,
  sortVms,
  vmBackendText,
  vmGpuText,
  vmOsText,
  vmProblem,
  vmSizeText,
  vmStateChip,
  type NodeVmFacts,
  type VmBackendChoice,
  type VmForm,
  type VmGpuChoice,
} from './workstations-vm-model';

export interface VmsSection extends Section {
  /** Opens the New VM form (after the nodes and VMs are read again). */
  openNew(): void;
}

/** A labelled control whose hint is a node, so a hint can change as the owner types. */
function labelled(label: string, control: Node, hint: Node): HTMLLabelElement {
  return h('label.field', null, h('span', null, label), control, hint);
}

/** The VMs the plaza made on its nodes, with the New VM form and the delete confirm. */
export function vmsSection(): VmsSection {
  let alive = true;
  // null: the last list request failed before any answer.
  let vms: StationVm[] | null = null;
  let nodes: NodeView[] | null = null;
  let closeDialog: (() => void) | null = null;
  const body = h('div', null, empty('clock', 'Loading…'));
  const el = card('Workstation VMs', body, { flush: true });

  const nodeName = (id: string): string => nodes?.find((n) => n.id === id)?.name || id;

  const render = (): void => {
    if (vms === null) return replace(body, empty('alert', 'Could not load the workstation VMs.'));
    if (!vms.length) {
      return replace(body, empty('machine', 'No workstation VMs', 'Make one with "New VM". It joins as a workstation by itself.'));
    }
    const columns: Column<StationVm>[] = [
      {
        label: 'Name',
        cell: (vm) => (vm.station ? h('a', { href: href('workstations', vm.station) }, h('strong', null, vm.name)) : h('strong', null, vm.name)),
      },
      { label: 'Node', cell: (vm) => nodeName(vm.node) },
      { label: 'OS', cell: (vm) => vmOsText(vm.os) },
      { label: 'Size', cell: (vm) => vmSizeText(vm) },
      { label: 'GPU', cell: (vm) => vmGpuText(vm.gpu) },
      { label: 'Backend', cell: (vm) => vmBackendText(vm.backend) },
      {
        label: 'State',
        cell: (vm) => {
          const chip = vmStateChip(vm);
          return badge(chip.label, chip.tone, true);
        },
      },
      {
        label: '',
        cell: (vm) =>
          h(
            'div.ws-buttons',
            null,
            vm.backend === 'plaza'
              ? button('Sleep', () => void sleep(vm), { size: 'sm', title: 'Keep its memory and stop it; it wakes in a fraction of a second' })
              : null,
            vm.backend === 'plaza' ? button('Wake', () => void wake(vm), { size: 'sm' }) : null,
            button('Delete', () => confirmRemove(vm), { kind: 'danger', size: 'sm' }),
          ),
      },
    ];
    replace(body, h('div.ws-scroll', null, table(columns, sortVms(vms))));
  };

  const refresh = async (): Promise<void> => {
    const [list, ns] = await Promise.all([api.stationVms().catch((): null => null), api.nodes().catch((): null => null)]);
    if (!alive) return;
    if (list) vms = list;
    if (ns) nodes = ns;
    render();
  };

  const confirmRemove = (vm: StationVm): void => {
    closeDialog?.();
    const pane = h(
      'div.stack',
      null,
      h('p', null, `Its VM goes from ${nodeName(vm.node)}, and the station it enrolled as is revoked. This cannot be undone.`),
      h(
        'div.ws-buttons',
        null,
        button('Cancel', () => close()),
        button('Delete', () => {
          close();
          void remove(vm);
        }, { kind: 'danger' }),
      ),
    );
    const close = modal(`Delete ${vm.name}?`, pane, () => {
      closeDialog = null;
    });
    closeDialog = close;
  };

  const remove = async (vm: StationVm): Promise<void> => {
    await attempt(() => api.deleteStationVm(vm.id), `${vm.name} deleted`);
    void refresh();
  };

  const sleep = async (vm: StationVm): Promise<void> => {
    await attempt(() => api.sleepStationVm(vm.id), `${vm.name} is asleep`);
    void refresh();
  };

  const wake = async (vm: StationVm): Promise<void> => {
    await attempt(() => api.wakeStationVm(vm.id), `${vm.name} woke`);
    void refresh();
  };

  /** The New VM form: fields that appear for the choices made, a live backend hint, and the hub's first problem. */
  const openForm = (): void => {
    const choices: NodeVmFacts[] = nodeChoices(nodes ?? []);
    const form: VmForm = { ...VM_FORM_DEFAULTS, node: choices[0]?.id ?? '' };
    let tried = false;
    let saving = false;

    const name = input({ placeholder: 'desk-1', maxlength: VM_NAME_MAX, autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Name' });
    const node = select(
      choices.length
        ? choices.map((c) => ({ value: c.id, label: nodeOptionLabel(c) }))
        : [{ value: '', label: nodes === null ? 'Could not load the nodes' : 'No node to run it on' }],
      form.node,
    );
    const nodeHint = h('span.hint');
    const os = select(
      [
        { value: 'linux', label: 'Linux' },
        { value: 'windows', label: 'Windows' },
        { value: 'macos', label: 'macOS (Apple hardware only)' },
      ],
      form.os,
    );
    const cpus = input({ type: 'number', min: VM_CPUS.min, max: VM_CPUS.max, step: 1, inputmode: 'numeric', value: form.cpus, 'aria-label': 'CPUs' });
    const ram = input({ type: 'number', min: VM_RAM_MB.min, max: VM_RAM_MB.max, step: 1, inputmode: 'numeric', value: form.ram_mb, 'aria-label': 'Memory in MiB' });
    const disk = input({ type: 'number', min: VM_DISK_GB.min, max: VM_DISK_GB.max, step: 1, inputmode: 'numeric', value: form.disk_gb, 'aria-label': 'Disk in GiB' });
    const gpu = select(
      [
        { value: 'none', label: 'None' },
        { value: 'virtual', label: 'Virtual GPU' },
        { value: 'vfio', label: 'Passthrough (vfio)' },
      ],
      form.gpu,
    );
    const pci = input({ placeholder: '0000:01:00.0', maxlength: 12, autocomplete: 'off', spellcheck: 'false', 'aria-label': 'PCI address' });
    const pciField = labelled(
      'PCI address',
      pci,
      h('span.hint', null, 'The device must already be bound to vfio-pci on the node. Its address, like 0000:01:00.0.'),
    );
    const backend = select(
      [
        { value: 'auto', label: 'Auto' },
        { value: 'plaza', label: 'plaza' },
        { value: 'qemu', label: 'QEMU' },
      ],
      form.backend,
    );
    const backendHintBox = h('span.hint');
    const image = input({ placeholder: '/srv/images/win.img or D:\\images\\win.img', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Windows image' });
    const imageField = labelled(
      'Windows image',
      image,
      h(
        'span.hint',
        null,
        'The path of a disk image on the node, copied for the VM and never downloaded. It must carry the virtio-win drivers: viosock is needed, or the VM cannot join.',
      ),
    );
    const problem = h('p.error-text');
    const submit = button('Make VM', () => void create(), { kind: 'primary' });

    /** Reads the controls into the form, then shows what follows from them. */
    const paint = (): void => {
      form.node = node.value;
      form.os = os.value as VmOs;
      form.name = name.value;
      form.cpus = cpus.value;
      form.ram_mb = ram.value;
      form.disk_gb = disk.value;
      form.gpu = gpu.value as VmGpuChoice;
      form.pci = pci.value;
      form.backend = backend.value as VmBackendChoice;
      form.image = image.value;
      const facts = choices.find((c) => c.id === form.node);
      nodeHint.textContent = nodeFactsLine(facts);
      pciField.hidden = !showsPci(form);
      imageField.hidden = !showsImage(form);
      backendHintBox.textContent = facts ? backendHint(form, facts.qemu) : 'Choose a node first.';
      // The first problem shows once the owner has pressed the button; before that the form only explains itself.
      if (tried) problem.textContent = vmProblem(form, facts, macosOn(vms ?? [], form.node)) ?? '';
    };

    const create = async (): Promise<void> => {
      if (saving) return;
      tried = true;
      paint();
      const facts = choices.find((c) => c.id === form.node);
      const read = readVmForm(form, facts, macosOn(vms ?? [], form.node));
      if (!read.ok) return;
      saving = true;
      submit.disabled = true;
      try {
        const made = await api.createStationVm(read.body);
        if (!alive) return;
        replace(pane, madeView(made, facts?.name ?? nodeName(made.node), form));
        void refresh();
      } catch (err) {
        problem.textContent = err instanceof Error ? err.message : String(err);
      } finally {
        saving = false;
        submit.disabled = false;
      }
    };

    /** The VM the hub made: what it is, the backend it got and why, and that it joins by itself. */
    const madeView = (made: StationVm, onNode: string, asked: VmForm): HTMLElement =>
      h(
        'div.stack',
        null,
        h('p', null, `Workstation VM ${made.name} (${made.id}) is being made on ${onNode}.`),
        h(
          'dl.node-facts',
          null,
          h('dt', null, 'OS'),
          h('dd', null, vmOsText(made.os)),
          h('dt', null, 'Size'),
          h('dd', null, vmSizeText(made)),
          h('dt', null, 'GPU'),
          h('dd', null, vmGpuText(made.gpu)),
          h('dt', null, 'Backend'),
          h('dd', null, `${vmBackendText(made.backend)}: ${backendReason(asked, made.backend)}.`),
        ),
        h('p.faint', null, 'It joins as a workstation by itself: when the VM starts, its daemon enrols with the join code it was given, and it appears in Workstations once it does.'),
        h('div.ws-buttons', null, button('Close', () => close(), { kind: 'primary' })),
      );

    const pane = h(
      'div.stack',
      null,
      h(
        'div.ws-form',
        null,
        labelled('Name', name, h('span.hint', null, `A DNS label: a-z, 0-9 and '-', up to ${VM_NAME_MAX} characters.`)),
        labelled('Node', node, nodeHint),
        labelled('OS', os, h('span.hint', null, 'macOS runs only on Apple hardware, two per Mac. Windows needs an image.')),
        labelled('CPUs', cpus, h('span.hint', null, `${VM_CPUS.min} to ${VM_CPUS.max}.`)),
        labelled('Memory (MiB)', ram, h('span.hint', null, `At least ${VM_RAM_MB.min} (${VM_WINDOWS_RAM_MB_MIN} for Windows), up to ${VM_RAM_MB.max}.`)),
        labelled('Disk (GiB)', disk, h('span.hint', null, `${VM_DISK_GB.min} to ${VM_DISK_GB.max}.`)),
        labelled('GPU', gpu, h('span.hint', null, 'None, a virtual GPU, or a real one given to the VM.')),
        pciField,
        labelled('Backend', backend, backendHintBox),
        imageField,
      ),
      problem,
      h('div.ws-buttons', null, submit),
    );
    for (const control of [name, node, os, cpus, ram, disk, gpu, pci, backend, image]) {
      control.addEventListener(control instanceof HTMLSelectElement ? 'change' : 'input', paint);
    }
    paint();
    closeDialog?.();
    const close = modal('New workstation VM', pane, () => {
      closeDialog = null;
    });
    closeDialog = close;
  };

  void refresh();
  return {
    el,
    refresh,
    openNew(): void {
      void (async () => {
        await refresh();
        if (alive) openForm();
      })();
    },
    dispose(): void {
      alive = false;
      closeDialog?.();
    },
  };
}
