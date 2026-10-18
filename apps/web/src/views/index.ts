// The screens, in sidebar order. Each `mount` renders into `root` and returns
// a cleanup that unsubscribes everything it subscribed to.

import type { IconName } from '../ui/icons.gen';
import { mount as agents } from './agents';
import { mount as approvals } from './approvals';
import { mount as board } from './board';
import { mount as conversations } from './conversations';
import { mount as e2b } from './e2b';
import { mount as volumes } from './volumes';
import { mount as egressCredentials } from './egress-credentials';
import { mount as council } from './council';
import { mount as files } from './files';
import { mount as machines } from './machines';
import { mount as history } from './history';
import { mount as recipes } from './recipes';
import { mount as hardwareLab } from './hardware-lab';
import { mount as lab } from './lab';
import { mount as robotLab } from './robot-lab';
import { mount as mcp } from './mcp';
import { mount as models } from './models';
import { mount as nodes } from './nodes';
import { mount as overview } from './overview';
import { mount as projects } from './projects';
import { mount as prompts } from './prompts';
import { mount as skills } from './skills';
import { mount as rules } from './rules';
import { mount as setup } from './setup';
import { mount as society } from './society';
import { mount as tasks } from './tasks';
import { mount as treasury } from './treasury';
import { mount as tools } from './tools';
import { mount as metrics } from './metrics';
import { mount as vault } from './vault';
import { mount as platforms } from './platforms';
import { mount as work } from './work';
import { mount as workstations } from './workstations';
import { lazyMount } from './lazy';

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
  { key: 'skills', label: 'Skills', icon: 'wrench', section: 'Plaza', mount: skills },
  { key: 'projects', label: 'Projects', icon: 'projects', section: 'Plaza', mount: projects },
  { key: 'work', label: 'Work', icon: 'commit', section: 'Plaza', mount: work },
  { key: 'tasks', label: 'Tasks', icon: 'tasks', section: 'Plaza', mount: tasks },
  { key: 'history', label: 'History', icon: 'git', section: 'Plaza', mount: history },
  { key: 'recipes', label: 'Recipes', icon: 'rules', section: 'Plaza', mount: recipes },
  { key: 'library', label: 'Library', icon: 'library', section: 'Plaza', mount: files },
  { key: 'approvals', label: 'Approvals', icon: 'approval', section: 'Plaza', mount: approvals },
  { key: 'machines', label: 'Machines', icon: 'machine', section: 'Plaza', mount: machines },
  { key: 'metrics', label: 'Metrics', icon: 'zap', section: 'Plaza', mount: metrics },
  { key: 'society', label: 'Society', icon: 'society', section: 'Social', mount: society },
  { key: 'conversations', label: 'Conversations', icon: 'conversations', section: 'Social', mount: conversations },
  { key: 'board', label: 'Board', icon: 'boardpin', section: 'Social', mount: board },
  { key: 'council', label: 'Council', icon: 'council', section: 'Social', mount: council },
  { key: 'treasury', label: 'Treasury', icon: 'treasury', section: 'Social', mount: treasury },
  { key: 'rules', label: 'Rules', icon: 'rules', section: 'Social', mount: rules },
  { key: 'lab', label: 'Lab', icon: 'brain', section: 'Lab', mount: lab },
  { key: 'models', label: 'Models', icon: 'models', section: 'Setup', mount: models },
  { key: 'tools', label: 'Tools', icon: 'wrench', section: 'Setup', mount: tools },
  { key: 'mcp', label: 'MCP', icon: 'terminal', section: 'Setup', mount: mcp },
  { key: 'vault', label: 'Vault', icon: 'key', section: 'Setup', mount: vault },
  { key: 'platforms', label: 'Git platforms', icon: 'git', section: 'Setup', mount: platforms },
  { key: 'egress', label: 'Egress credentials', icon: 'key', section: 'Setup', mount: egressCredentials },
  { key: 'e2b', label: 'E2B API', icon: 'wrench', section: 'Setup', mount: e2b },
  { key: 'volumes', label: 'Volumes', icon: 'wrench', section: 'Setup', mount: volumes },
  { key: 'workstations', label: 'Workstations', icon: 'harness', section: 'Setup', mount: workstations },
  { key: 'nodes', label: 'Nodes', icon: 'link', section: 'Setup', mount: nodes },
  { key: 'setup', label: 'Setup', icon: 'wrench', section: 'Setup', mount: setup },
  { key: 'hardware-lab', label: 'Hardware lab', icon: 'wrench', section: 'Setup', mount: hardwareLab, hidden: true },
  { key: 'robot-lab', label: 'Robot lab', icon: 'wrench', section: 'Setup', mount: robotLab, hidden: true },
  // The IDE is reached by its route and openIde (ide/open.ts), not the sidebar; its code loads on the first visit.
  { key: 'ide', label: 'IDE', icon: 'file', section: 'Plaza', mount: lazyMount(() => import('./ide'), (m) => m.mount), hidden: true },
];
