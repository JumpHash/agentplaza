// Robot navigation for the board plaza (docs/design/navigation.md). Pure: no three.js. The module is split by
// concern: the contracts (nav-types), the grid built from the board and its rooms (nav-grid), the planner
// (nav-plan: A*, string-pulling, corner cutting) and the robots that follow paths and avoid each other
// (nav-agents). The floor calls NavWorld.setGoal and NavWorld.step; it reads NavWorld.pose each frame.

export * from './nav-types';
export * from './nav-grid';
export * from './nav-plan';
export * from './nav-agents';

import type { NavGoal } from './nav-types';
import type { NavWorld } from './nav-agents';

/**
 * The movement seam for robot activities: walk robot `id` to `goal` (a world point, with an optional arrival yaw) by
 * planned, avoiding, door-aware paths. Replaces any straight-line walk. A goal with no path leaves the robot where it
 * is (pose.moving stays false), so callers may retry. Per frame, the floor calls world.step(dt) and reads world.pose(id).
 */
export function navigateTo(world: NavWorld, id: number, goal: NavGoal): void {
  world.setGoal(id, goal);
}
