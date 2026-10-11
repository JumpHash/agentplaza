// The screens, in sidebar order. Each `mount` renders into `root` and returns
// a cleanup that unsubscribes everything it subscribed to.

import type { IconName } from '../ui/icons.gen';
import { mount as agents } from './agents';
import { mount as board } from './board';
import { mount as conversations } from './conversations';
import { mount as council } from './council';
import { mount as floor } from './floor';
import { mount as machine } from './machine';
import { mount as models } from './models';
import { mount as overview } from './overview';
import { mount as tasks } from './tasks';
import { mount as treasury } from './treasury';

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
  { key: 'tasks', label: 'Tasks', icon: 'tasks', section: 'Plaza', mount: tasks },
  { key: 'floor', label: 'Floor', icon: 'floor', section: 'Plaza', mount: floor },
  { key: 'conversations', label: 'Conversations', icon: 'conversations', section: 'Social', mount: conversations },
  { key: 'board', label: 'Board', icon: 'board', section: 'Social', mount: board },
  { key: 'council', label: 'Council', icon: 'council', section: 'Social', mount: council },
  { key: 'treasury', label: 'Treasury', icon: 'treasury', section: 'Social', mount: treasury },
  { key: 'models', label: 'Models', icon: 'models', section: 'Setup', mount: models },
  { key: 'machine', label: 'Machines', icon: 'machine', section: 'Setup', mount: machine },
];
