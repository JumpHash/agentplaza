// The sign-in screen: a hub off loopback wants the owner's token once. The token
// goes to the hub, which answers with an HttpOnly session cookie; the page never keeps it.

import { api, ApiError } from '../api';
import { brandMark } from '../ui/brand';
import { button, field, input } from '../ui/components';
import { h, replace } from '../ui/dom';

export function signInScreen(onSignedIn: () => void): HTMLElement {
  const who = h('p.signin-hub');
  api.hubIdentity().then(
    (id) => replace(who, h('span', null, id.name), id.fingerprint ? [' · ', h('span', null, `${id.fingerprint.slice(0, 16)}…`)] : null),
    () => who.remove(),
  );

  const token = input({ type: 'password', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false', 'aria-label': 'Owner token' });
  const error = h('p.signin-error', { role: 'alert' });
  const submit = button('Sign in', () => undefined, { kind: 'primary' });
  submit.type = 'submit';

  let busy = false;
  async function signIn(): Promise<void> {
    if (busy) return;
    if (!token.value) {
      error.textContent = 'Paste the owner token first.';
      token.focus();
      return;
    }
    busy = true;
    error.textContent = '';
    sheet.setAttribute('aria-busy', 'true');
    token.disabled = true;
    submit.disabled = true;
    submit.textContent = 'Signing in…';
    try {
      await api.signIn(token.value);
      token.value = '';
      onSignedIn();
    } catch (err) {
      error.textContent = err instanceof ApiError ? err.message : 'The hub did not answer. Try again.';
      token.focus();
    } finally {
      busy = false;
      sheet.removeAttribute('aria-busy');
      token.disabled = false;
      submit.disabled = false;
      submit.textContent = 'Sign in';
    }
  }

  const sheet = h(
    'form.signin-sheet',
    {
      onsubmit: (ev: Event) => {
        ev.preventDefault();
        void signIn();
      },
    },
    h('header.signin-brand', null, brandMark(36), h('span.signin-name', null, 'AgentPlaza')),
    h('h1', null, 'Sign in to this hub'),
    h('p.signin-lede', null, "Paste the owner token. The hub keeps it in the owner-token file in its state directory, on the hub's own machine; `agentplaza hub login` reads it from there."),
    who,
    field('Owner token', token),
    error,
    h('div.signin-actions', null, submit),
  );
  return h('div.signin', null, sheet);
}
