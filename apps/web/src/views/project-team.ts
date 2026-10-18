// A project's team: the agents working on it, the bench to add from, and the agents that held its tasks before it had
// members (one step to put them all on it). New agents can be spawned for it; they join at their first hello.

import { api, type Agent, type Project, type SuggestedMember } from '../api';
import { href } from '../router';
import { agentName, roleLabel } from '../roles';
import { isLive } from '../scope';
import { refreshAgents } from '../store';
import { attempt, badge, button, card } from '../ui/components';
import { h, replace } from '../ui/dom';
import { avatar } from './agent-profile';
import { openAgentWizard } from './agent-wizard';
import { statusPill } from './agent-list';

/** One agent's row: who, what role, how it is, and its action. */
function row(a: Agent, action: HTMLElement | null, note?: string): HTMLElement {
  const pill = statusPill(a.status);
  return h(
    'div.row.team-row',
    null,
    avatar(a, 28),
    h(
      'div.team-who',
      null,
      h('a.truncate', { href: href('agents', a.cid) }, h('strong', null, agentName(a))),
      h('div.faint.truncate', null, roleLabel(a.role), note ? ` · ${note}` : ''),
    ),
    badge(pill.word ?? a.status, pill.tone),
    action,
  );
}

/** The Team tab of project `p`. `changed` redraws the page after a move. */
export function teamTab(p: Project, changed: () => void): HTMLElement {
  const root = h('div.pj-sections');
  const live = isLive(p);
  const draw = async (): Promise<void> => {
    const [members, bench, suggested] = await Promise.all([
      attempt(() => api.projectMembers(p.id)),
      live ? attempt(() => api.agentsOf('bench')) : Promise.resolve([] as Agent[]),
      live ? attempt(() => api.suggestedMembers(p.id)) : Promise.resolve([] as SuggestedMember[]),
    ]);
    const moved = (): void => {
      void refreshAgents();
      void draw();
      changed();
    };
    const remove = (a: Agent): HTMLElement =>
      button('Back to the bench', () => void attempt(() => api.removeMember(p.id, a.cid), `${agentName(a)} is back on the bench`).then(moved), { size: 'sm', kind: 'ghost' });
    const add = (a: Agent): HTMLElement =>
      button('Add', () => void attempt(() => api.addMember(p.id, a.cid), `${agentName(a)} joined ${p.title}`).then(moved), { size: 'sm', icon: 'plus' });
    const addAll = (list: SuggestedMember[]): HTMLElement =>
      button(
        list.length > 1 ? `Add all ${list.length}` : 'Add',
        () =>
          void (async () => {
            for (const a of list) await attempt(() => api.addMember(p.id, a.cid));
            moved();
          })(),
        { size: 'sm', kind: 'primary', icon: 'plus' },
      );
    const sections: HTMLElement[] = [
      card(
        `Working on it (${members?.length ?? 0})`,
        members?.length
          ? h('div.stack', null, members.map((a) => row(a, live ? remove(a) : null, a.held ?? undefined)))
          : h('p.pj-none', null, live ? 'Nobody yet: add agents from the bench, or spawn new ones for this project.' : 'Nobody: the project is over, and its agents went back to the bench.'),
        {
          actions: live
            ? [button('New agents for this project', () => openAgentWizard({ project: p.id, onDone: moved }), { size: 'sm', icon: 'plus' })]
            : [],
        },
      ),
    ];
    if (suggested?.length) {
      sections.push(
        card(
          'Worked on its tasks before',
          h(
            'div.stack',
            null,
            h('p.faint', null, 'These agents hold tasks of this project from before agents worked on one project at a time. Put them on it, and its tasks run again.'),
            suggested.map((a) => row(a, add(a), `holds ${a.tasks.join(', ')}`)),
          ),
          { actions: [addAll(suggested)] },
        ),
      );
    }
    if (live) {
      sections.push(
        card(
          'The bench',
          bench?.length
            ? h('div.stack', null, h('p.faint', null, 'Agents on no project do nothing, and sleep. Adding one stops nothing: it has no work.'), bench.map((a) => row(a, add(a))))
            : h('p.pj-none', null, 'Nobody is on the bench.'),
        ),
      );
    }
    replace(root, sections);
  };
  void draw();
  return root;
}
