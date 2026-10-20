// The sign-in screen: a hub off loopback wants the owner's token once. The token
// goes to the hub, which answers with an HttpOnly session cookie; the page never keeps it.
// The hub's identity is pinned for this origin at the first successful sign-in (hub-pin.ts). When
// a later visit finds another identity, the token is not sent until the owner trusts the new one.

import '../styles/proto/onboard.css';
import { api, ApiError } from '../api';
import type { HubIdentity } from '../api';
import { brandMark } from '../ui/brand';
import { button, field, input } from '../ui/components';
import { h, replace } from '../ui/dom';
import { checkPin, forgetPin, type HubPin, pinFor, setPin, shortFingerprint } from '../hub-pin';

export function signInScreen(onSignedIn: () => void): HTMLElement {
  const origin = location.origin;
  const who = h('p.signin-hub');
  const pinLine = h('p.signin-hub');
  const warning = h('div.signin-warning', { role: 'alert' });
  warning.hidden = true;

  const token = input({ id: 'owner-token', type: 'password', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false', 'aria-label': 'Owner token' });
  const error = h('p.signin-error', { id: 'sign-msg', role: 'alert' });
  const submit = button('Sign in', () => undefined, { kind: 'primary' });
  submit.id = 'sign-go';
  submit.type = 'submit';

  // The token is held back until the hub's identity is checked: a pinned origin whose identity is
  // unreadable or different never receives it.
  let blocked = true;
  let seen: HubIdentity | null = null;
  let busy = false;

  function setBlocked(on: boolean): void {
    blocked = on;
    token.disabled = on;
    submit.disabled = on;
  }

  function setLabel(text: string): void {
    const label = submit.querySelector('span');
    if (label) label.textContent = text;
  }

  function renderPinLine(): void {
    const pin = pinFor(origin);
    if (!pin) {
      pinLine.replaceChildren();
      return;
    }
    pinLine.replaceChildren(
      h('span', null, `Pinned since ${pin.since}`),
      ' · ',
      button(
        'Forget pin',
        () => {
          forgetPin(origin);
          renderPinLine();
        },
        { kind: 'ghost', size: 'sm' },
      ),
    );
  }

  function showMismatch(pin: HubPin, received: string | null): void {
    setBlocked(true);
    const trust = button(
      'Trust the new identity',
      () => {
        if (received) setPin(origin, received);
        else forgetPin(origin);
        warning.hidden = true;
        error.textContent = '';
        setBlocked(false);
        renderPinLine();
      },
      { kind: 'primary', size: 'sm' },
    );
    const cancel = button(
      'Cancel',
      () => {
        warning.hidden = true;
        error.textContent = "Not signed in: this hub's identity is not trusted. Reload the page to check it again.";
      },
      { kind: 'ghost', size: 'sm' },
    );
    warning.replaceChildren(
      h(
        'p',
        null,
        `This hub answers with a different identity. Pinned ${shortFingerprint(pin.fingerprint)}, received ${shortFingerprint(received)}. A reinstalled hub has a new identity; anything else answering here is not your hub. No token is sent until you trust the new identity.`,
      ),
      h('div.signin-actions', null, trust, cancel),
    );
    warning.hidden = false;
    renderPinLine();
  }

  api.hubIdentity().then(
    (id) => {
      seen = id;
      replace(who, h('span', null, id.name), id.fingerprint ? [' · ', h('span.mono', null, `${id.fingerprint.slice(0, 16)}…`)] : null);
      const check = checkPin(origin, id);
      if (check.kind === 'mismatch') {
        showMismatch(check.pin, check.received);
      } else {
        setBlocked(false);
      }
      renderPinLine();
    },
    () => {
      who.remove();
      if (pinFor(origin)) {
        error.textContent = "Could not read this hub's identity, so the owner token is not sent. Reload the page to try again.";
      } else {
        setBlocked(false);
      }
    },
  );

  async function signIn(): Promise<void> {
    if (busy || blocked) return;
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
    setLabel('Signing in…');
    try {
      await api.signIn(token.value);
      // The first successful sign-in pins the identity that was checked.
      if (seen?.fingerprint && !pinFor(origin)) setPin(origin, seen.fingerprint);
      token.value = '';
      onSignedIn();
    } catch (err) {
      error.textContent = err instanceof ApiError ? err.message : 'The hub did not answer. Try again.';
      token.focus();
    } finally {
      busy = false;
      sheet.removeAttribute('aria-busy');
      token.disabled = blocked;
      submit.disabled = blocked;
      setLabel('Sign in');
    }
  }

  const sheet = h(
    'form.signin-sheet',
    {
      novalidate: true,
      onsubmit: (ev: Event) => {
        ev.preventDefault();
        void signIn();
      },
    },
    h('header.signin-brand', null, brandMark(28), h('span.signin-name', null, 'AgentPlaza')),
    h('h1', null, 'Sign in to this hub'),
    h(
      'p.signin-lede',
      null,
      "Paste the owner token. The hub keeps it in the owner-token file in its state directory, on the hub's own machine; ",
      h('code', null, 'agentplaza hub login'),
      ' reads it from there.',
    ),
    who,
    pinLine,
    warning,
    field('Owner token', token),
    error,
    h('div.signin-actions', null, submit),
  );
  return h('div.signin', null, sheet);
}
