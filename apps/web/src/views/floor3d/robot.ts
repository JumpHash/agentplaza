// A plaza robot: a small, cute, WALL·E-inspired 3D model (boxy body, tank
// treads, binocular eyes on a neck, little arms) in the plaza's palette.
// The scene owns placement; the robot owns its own animation.

import * as THREE from 'three';

export type RobotState = 'idle' | 'working' | 'walking' | 'offline';

export interface Palette {
  ink: string; sheet: string; paper: string; rule: string; signal: string; ok: string; info: string; inkMuted: string;
}

export interface Robot {
  /** Add this to the scene; position/rotate it from the scene. */
  readonly group: THREE.Group;
  /** World-space point just above the head, for the status label. */
  labelAnchor(target: THREE.Vector3): THREE.Vector3;
  setState(state: RobotState): void;
  /** Advance the animation: `dt` seconds since the last frame, `t` seconds since start. */
  update(dt: number, t: number): void;
  setHighlighted(on: boolean): void;
  dispose(): void;
}

/** `hue` (0–360) tints the body panel; the orchestrator is larger with a signal-orange trim. */
export function makeRobot(hue: number, orchestrator: boolean, palette: Palette): Robot {
  void hue; void orchestrator; void palette;
  throw new Error('makeRobot: not implemented');
}
