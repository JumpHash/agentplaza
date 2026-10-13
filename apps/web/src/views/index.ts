// The screens, in sidebar order. Each `mount` renders into `root` and returns
// a cleanup that unsubscribes everything it subscribed to.

import type { IconName } from '../ui/icons.gen';
import { mount as agents } from './agents';
import { mount as approvals } from './approvals';
import { mount as board } from './board';
import { mount as conversations } from './conversations';
import { mount as council } from './council';
import { mount as files } from './files';
import { mount as floor } from './floor';
import { mount as git } from './git';
import { mount as hardwareLab } from './hardware-lab';
import { mount as lab } from './lab';
import { mount as robotLab } from './robot-lab';
import { mount as roomLab } from './room-lab';
import { mount as machine } from './machine';
import { mount as mcp } from './mcp';
import { mount as models } from './models';
import { mount as nodes } from './nodes';
import { mount as overview } from './overview';
import { mount as projects } from './projects';
import { mount as prompts } from './prompts';
import { mount as rules } from './rules';
import { mount as setup } from './setup';
import { mount as society } from './society';
import { mount as tasks } from './tasks';
import { mount as treasury } from './treasury';
import { mount as tools } from './tools';
import { mount as usage } from './usage';
import { mount as vault } from './vault';

export type Cleanup = () => void;
export type Mount = (root: HTMLElement, params: string[]) => Cleanup;

export interface Screen {
  key: string;
  label: string;
  icon: IconName;
  section: string;
  mount: Mount;
  /** Reachable by its route, not listed in the sidebar (developer tools). */
  hidden?: boolean;
}

export const SCREENS: Screen[] = [
  { key: 'overview', label: 'Overview', icon: 'overview', section: 'Plaza', mount: overview },
  { key: 'agents', label: 'Agents', icon: 'agents', section: 'Plaza', mount: agents },
  { key: 'prompts', label: 'Prompts', icon: 'edit', section: 'Plaza', mount: prompts },
  { key: 'projects', label: 'Projects', icon: 'projects', section: 'Plaza', mount: projects },
  { key: 'tasks', label: 'Tasks', icon: 'tasks', section: 'Plaza', mount: tasks },
  { key: 'git', label: 'Git', icon: 'git', section: 'Plaza', mount: git },
  { key: 'library', label: 'Library', icon: 'library', section: 'Plaza', mount: files },
  { key: 'approvals', label: 'Approvals', icon: 'approval', section: 'Plaza', mount: approvals },
  { key: 'floor', label: 'Floor', icon: 'floor', section: 'Plaza', mount: floor },
  { key: 'society', label: 'Society', icon: 'society', section: 'Social', mount: society },
  { key: 'conversations', label: 'Conversations', icon: 'conversations', section: 'Social', mount: conversations },
  { key: 'board', label: 'Board', icon: 'boardpin', section: 'Social', mount: board },
  { key: 'council', label: 'Council', icon: 'council', section: 'Social', mount: council },
  { key: 'treasury', label: 'Treasury', icon: 'treasury', section: 'Social', mount: treasury },
  { key: 'rules', label: 'Rules', icon: 'rules', section: 'Social', mount: rules },
  { key: 'lab', label: 'Lab', icon: 'brain', section: 'Lab', mount: lab },
  { key: 'models', label: 'Models', icon: 'models', section: 'Setup', mount: models },
  { key: 'usage', label: 'Usage', icon: 'zap', section: 'Setup', mount: usage },
  { key: 'tools', label: 'Tools', icon: 'wrench', section: 'Setup', mount: tools },
  { key: 'mcp', label: 'MCP', icon: 'terminal', section: 'Setup', mount: mcp },
  { key: 'vault', label: 'Vault', icon: 'key', section: 'Setup', mount: vault },
  { key: 'machine', label: 'Machines', icon: 'machine', section: 'Setup', mount: machine },
  { key: 'nodes', label: 'Nodes', icon: 'link', section: 'Setup', mount: nodes },
  { key: 'setup', label: 'Setup', icon: 'wrench', section: 'Setup', mount: setup },
  { key: 'hardware-lab', label: 'Hardware lab', icon: 'wrench', section: 'Setup', mount: hardwareLab, hidden: true },
  { key: 'robot-lab', label: 'Robot lab', icon: 'wrench', section: 'Setup', mount: robotLab, hidden: true },
  { key: 'room-lab', label: 'Room lab', icon: 'wrench', section: 'Setup', mount: roomLab, hidden: true },
];
