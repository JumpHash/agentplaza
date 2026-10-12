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
import { mount as machine } from './machine';
import { mount as models } from './models';
import { mount as overview } from './overview';
import { mount as projects } from './projects';
import { mount as society } from './society';
import { mount as tasks } from './tasks';
import { mount as treasury } from './treasury';
import { mount as tools } from './tools';

export type Cleanup = () => void;
export type Mount = (root: HTMLElement, params: string[]) => Cleanup;

export interface Screen {
  key: string;
  label: string;
  icon: IconName;
  section: string;
  mount: Mount;
}

export const SCREENS: Screen[] = [
  { key: 'overview', label: 'Overview', icon: 'overview', section: 'Plaza', mount: overview },
  { key: 'agents', label: 'Agents', icon: 'agents', section: 'Plaza', mount: agents },
  { key: 'projects', label: 'Projects', icon: 'projects', section: 'Plaza', mount: projects },
  { key: 'tasks', label: 'Tasks', icon: 'tasks', section: 'Plaza', mount: tasks },
  { key: 'git', label: 'Git', icon: 'git', section: 'Plaza', mount: git },
  { key: 'library', label: 'Library', icon: 'board', section: 'Plaza', mount: files },
  { key: 'approvals', label: 'Approvals', icon: 'alert', section: 'Plaza', mount: approvals },
  { key: 'floor', label: 'Floor', icon: 'floor', section: 'Plaza', mount: floor },
  { key: 'society', label: 'Society', icon: 'society', section: 'Social', mount: society },
  { key: 'conversations', label: 'Conversations', icon: 'conversations', section: 'Social', mount: conversations },
  { key: 'board', label: 'Board', icon: 'board', section: 'Social', mount: board },
  { key: 'council', label: 'Council', icon: 'council', section: 'Social', mount: council },
  { key: 'treasury', label: 'Treasury', icon: 'treasury', section: 'Social', mount: treasury },
  { key: 'models', label: 'Models', icon: 'models', section: 'Setup', mount: models },
  { key: 'tools', label: 'Tools', icon: 'zap', section: 'Setup', mount: tools },
  { key: 'machine', label: 'Machines', icon: 'machine', section: 'Setup', mount: machine },
];
