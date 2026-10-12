// The model picker: a styled dropdown of models grouped by provider, used wherever a
// model is chosen (agent page, spawn dialog, model policy). Only the models a caller
// passes are offered — callers pass what the agent's tier allows, so nothing listed
// is refused.

export interface ModelChoice {
  /** What is stored: `provider/model[@effort]`, or '' for an "inherit" choice. */
  value: string;
  /** The model's display name (falls back to its id). */
  label: string;
  /** The provider id the choice is grouped under; '' for the inherit choice (shown first, ungrouped). */
  provider: string;
  /** A short faint note, e.g. "low effort" or "current". */
  hint?: string;
}

export interface ModelPicker {
  /** The control to place in the page. */
  el: HTMLElement;
  /** The selected value. Setting it does not fire `onChange`. */
  value: string;
  /** Replaces the choices, keeping the value when it is still offered. */
  setChoices(choices: ModelChoice[]): void;
  disabled: boolean;
}

export interface ModelPickerOptions {
  choices: ModelChoice[];
  value?: string;
  /** Shown when nothing is selected and there is no inherit choice. */
  placeholder?: string;
  /** Called when the user picks a different choice. */
  onChange?: (value: string) => void;
  /** Accessible name. */
  label?: string;
}

/** Builds choices from `provider/model[@effort]` strings, naming them from `names` (`provider/model` → display name). */
export function choicesFrom(values: string[], names: Map<string, string>, inherit?: { label: string }): ModelChoice[] {
  // contract: M1
  void values;
  void names;
  void inherit;
  return [];
}

export function modelPicker(_opts: ModelPickerOptions): ModelPicker {
  // contract: M1
  const el = document.createElement('div');
  return { el, value: '', setChoices: () => {}, disabled: false };
}
