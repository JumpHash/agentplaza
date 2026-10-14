// A machine's look: its kind, with Apple's laptop and desktop told apart (the device pictures in ui/devices.ts).
/** A machine's kind, as the fleet reports it. */
export type MachineKind = 'server' | 'workstation' | 'laptop' | 'edge' | 'phone';

export type Look = MachineKind | 'macbook' | 'mac';

/** The look of a machine of `kind` made by `vendor`. */
export function lookOf(kind: MachineKind, vendor?: string | null): Look {
  if (vendor !== 'apple' || kind === 'phone') return kind;
  return kind === 'laptop' ? 'macbook' : 'mac';
}
