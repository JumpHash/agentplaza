// Models > Decisions: the decision model the plaza decides with (saved to /v1/decisions/default),
// and a Try-it form that asks one typed question of it.

import { api, type DecisionAnswer, type DecisionDefault, type DecisionQuestion, type ModelInfo } from '../api';
import { attempt, button, card, field, input, select, textarea, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { choicesFrom, modelPicker } from '../ui/model-picker';

export interface DecisionPanel {
  el: HTMLElement;
  /** Re-reads the decision models and the default (after a provider was added, edited or removed). */
  reload(): Promise<void>;
}

const pct = (p: number): string => `${Math.round(Math.min(1, Math.max(0, p)) * 100)}%`;
const split = (text: string): string[] => text.split(',').map((s) => s.trim()).filter(Boolean);

/** One labelled probability bar. */
function bar(label: string, p: number): HTMLElement {
  const fill = h('span.dec-fill');
  fill.style.width = pct(p);
  return h('div.dec-row', null, h('span.dec-label', null, label), h('span.dec-track', null, fill), h('span.dec-pct.mono', null, pct(p)));
}

/** One answer from the plaza's response: a probability, a chosen value with its options, a scored level, or a refusal. */
function answerView(a: DecisionAnswer): HTMLElement {
  const key = { 'data-key': `answer-${a.name}` };
  if (a.type === 'refusal') {
    return h('div.dec-answer', key, h('strong', null, a.name), h('p.dec-refusal', null, 'Refused: the model declined to answer this question.'));
  }
  if (a.type === 'predicate') {
    return h('div.dec-answer', key, h('strong', null, a.name), bar('Probability', a.probability));
  }
  if (a.type === 'choice') {
    return h(
      'div.dec-answer',
      key,
      h('strong', null, a.name),
      h('p.dec-verdict', null, `Chose ${a.choice}`, h('span.faint', null, ` · confidence ${pct(a.confidence)}`)),
      ...a.probabilities.map((p) => bar(p.value, p.probability)),
    );
  }
  const level = a.probabilities.find((p) => p.value === Math.round(a.score));
  return h(
    'div.dec-answer',
    key,
    h('strong', null, a.name),
    h('p.dec-verdict', null, `Score ${a.score.toFixed(2)}${level ? ` · ${level.label}` : ''}`, h('span.faint', null, ` · confidence ${pct(a.confidence)}`)),
    ...a.probabilities.map((p) => bar(p.label, p.probability)),
  );
}

export function decisionPanel(): DecisionPanel {
  let models: ModelInfo[] = [];
  let current: DecisionDefault = { provider: null, model: null };
  const pickerSlot = h('div.dec-picker');

  const choose = async (value: string): Promise<void> => {
    // Split on the first slash only: the model part of an id can hold slashes too.
    const at = value.indexOf('/');
    if (at < 0) return;
    const saved = await attempt(
      () => api.setDecisionDefault({ provider: value.slice(0, at), model: value.slice(at + 1) }),
      'Decision model saved',
    );
    if (saved) current = saved;
    else await load();
  };

  const draw = (): void => {
    if (!models.length) {
      replace(pickerSlot, h('p.faint', null, 'No decision models yet. Add a decision provider below.'));
      return;
    }
    const values = models.map((m) => `${m.provider}/${m.id}`);
    const names = new Map(models.map((m, i) => [values[i], m.name ?? m.id] as [string, string]));
    const hasDefault = current.provider !== null && current.model !== null;
    const picker = modelPicker({
      choices: choicesFrom(values, names),
      value: hasDefault ? `${current.provider}/${current.model}` : '',
      placeholder: 'Choose a decision model',
      label: 'Decision model',
      onChange: (v) => void choose(v),
    });
    replace(pickerSlot, picker.el);
  };

  const load = async (): Promise<void> => {
    const [view, def] = await Promise.all([attempt(() => api.decisionModels()), attempt(() => api.getDecisionDefault())]);
    if (view) models = view.models;
    if (def) current = def;
    draw();
  };

  // ---- Try it: one question, asked of the decision model ----
  const kind = select(
    [
      { value: 'predicate', label: 'Predicate: a yes/no probability' },
      { value: 'choice', label: 'Choice: one of several values' },
      { value: 'score', label: 'Score: a level on a scale' },
    ],
    'predicate',
  );
  const text = textarea({ rows: 3, placeholder: 'The input the question is about' });
  const instructions = textarea({ rows: 2, placeholder: 'What to decide, e.g. Is this message a complaint?' });
  const choicesList = input({ placeholder: 'yes, no, unsure', autocomplete: 'off' });
  const levelsList = input({ placeholder: 'low, medium, high', autocomplete: 'off' });
  const choicesField = field('Choices', choicesList, 'Comma-separated, at least two');
  const levelsField = field('Levels', levelsList, 'Comma-separated, lowest first, two to ten');
  const sync = (): void => {
    choicesField.hidden = kind.value !== 'choice';
    levelsField.hidden = kind.value !== 'score';
  };
  kind.addEventListener('change', sync);
  sync();

  const question = (): DecisionQuestion | string => {
    const instr = instructions.value.trim();
    if (kind.value === 'choice') {
      const values = split(choicesList.value);
      if (values.length < 2) return 'Give at least two choices';
      return { type: 'choice', name: 'question', instructions: instr, choices: values.map((value) => ({ value })) };
    }
    if (kind.value === 'score') {
      const labels = split(levelsList.value);
      if (labels.length < 2 || labels.length > 10) return 'Give two to ten levels';
      return { type: 'score', name: 'question', instructions: instr, levels: labels.map((label) => ({ label })) };
    }
    return { type: 'predicate', name: 'question', instructions: instr };
  };

  const result = h('div.dec-result');
  const ask = button(
    'Ask',
    async () => {
      const { provider, model } = current;
      if (!provider || !model) {
        toast('Choose a decision model first', 'error');
        return;
      }
      const body = text.value.trim();
      if (!body) {
        toast('Write the input', 'error');
        return;
      }
      const q = question();
      if (typeof q === 'string') {
        toast(q, 'error');
        return;
      }
      ask.disabled = true;
      const res = await attempt(() => api.askDecision({ provider, model, input: body, questions: [q] }));
      ask.disabled = false;
      if (!res) return;
      replace(
        result,
        h('p.faint.mono', null, `${res.model} · ${res.usage.input_tokens} input tokens`),
        ...res.answers.map(answerView),
      );
    },
    { kind: 'primary' },
  );

  const el = h(
    'div.dec-panel',
    null,
    card('Decision model', [
      h('p.faint', null, 'The model the plaza decides with. Agents ask it through the plaza; they never chat with it.'),
      pickerSlot,
    ]),
    card('Try it', [
      h(
        'div.dec-form',
        null,
        field('Input', text),
        field('Question type', kind),
        field('Instructions', instructions),
        choicesField,
        levelsField,
        h('div.dec-actions', null, ask),
      ),
      result,
    ]),
  );

  void load();
  return { el, reload: load };
}
