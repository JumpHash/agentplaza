// New project: talk it through with the orchestrator (a conversation in the IDE, where it drafts the project beside the
// chat and the owner starts it), or open one directly with a title and a goal.

import { api } from '../api';
import { openConversation } from '../ide/open';
import { hrefIn } from '../router';
import { refreshProjects } from '../scope';
import { attempt, button, field, input, modal, select, textarea, toast } from './components';
import { h } from './dom';

export function openNewProject(): void {
  const ask = textarea({ rows: 4, placeholder: 'What do you want done? “The scheduler tests are flaky and I want 0.9 out this week.”' });
  const talk = button(
    'Start a conversation',
    () => {
      const text = ask.value.trim();
      if (!text) {
        toast('Say what you want done first', 'error');
        return;
      }
      void (async () => {
        const conversation = await attempt(() => api.openConversation(text));
        if (!conversation) return;
        close();
        openConversation(conversation.id);
      })();
    },
    { kind: 'primary', icon: 'conversations' },
  );
  const title = input({ placeholder: 'Release 0.9' });
  const goal = textarea({ rows: 3, placeholder: 'What the project delivers' });
  const kind = select(
    [
      { value: 'code', label: 'Code (it gets a repository)' },
      { value: 'other', label: 'Other' },
    ],
    'code',
  );
  const direct = button('Open project', () => {
    const t = title.value.trim();
    const g = goal.value.trim();
    if (!t || !g) {
      toast('A project needs a title and a goal', 'error');
      return;
    }
    void (async () => {
      const project = await attempt(() => api.createProject({ title: t, goal: g, kind: kind.value as 'code' | 'other' }), `Project ${t} opened`);
      if (!project) return;
      close();
      await refreshProjects();
      location.hash = hrefIn(project.id, 'projects', project.id);
    })();
  });
  const close = modal(
    'New project',
    h(
      'div.new-project',
      null,
      h(
        'section',
        null,
        h('h3', null, 'Talk it through'),
        h('p.faint', null, 'The orchestrator asks what it needs and drafts the project beside the conversation: its tasks, its team and its budget. You start it when it looks right.'),
        field('Your goal', ask),
        h('div.actions', null, talk),
      ),
      h(
        'section',
        null,
        h('h3', null, 'Or open it directly'),
        h('p.faint', null, 'An empty project of yours: you put agents on it and give it tasks.'),
        field('Title', title),
        field('Goal', goal),
        field('Kind', kind),
        h('div.actions', null, direct),
      ),
    ),
  );
}
