// Trust on first use for the web console. The hub's identity (its TLS certificate fingerprint) is
// remembered per origin in localStorage the first time a sign-in succeeds. A later visit that finds
// another identity is stopped before the owner token is sent. Storage can be unavailable (private
// windows, blocked site data); then nothing is remembered and each visit is a first use.

import type { HubIdentity } from './api';

export const PIN_KEY = 'apz.hubPins';

export interface HubPin {
  fingerprint: string;
  /** The UTC day the identity was pinned, YYYY-MM-DD. */
  since: string;
}

type Pins = Record<string, HubPin>;

export type PinCheck =
  | { kind: 'first-use' }
  | { kind: 'match'; pin: HubPin }
  | { kind: 'mismatch'; pin: HubPin; received: string | null };

function readPins(): Pins {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(PIN_KEY) ?? '{}');
    return parsed && typeof parsed === 'object' ? (parsed as Pins) : {};
  } catch {
    return {};
  }
}

function writePins(pins: Pins): void {
  try {
    localStorage.setItem(PIN_KEY, JSON.stringify(pins));
  } catch {
    // Storage is unavailable: the pin lasts for this page only.
  }
}

function today(): string {
  return new Date().toISOString().slice(0, 10);
}

export function pinFor(origin: string): HubPin | undefined {
  return readPins()[origin];
}

export function setPin(origin: string, fingerprint: string): HubPin {
  const pin: HubPin = { fingerprint, since: today() };
  const pins = readPins();
  pins[origin] = pin;
  writePins(pins);
  return pin;
}

export function forgetPin(origin: string): void {
  const pins = readPins();
  delete pins[origin];
  writePins(pins);
}

/** Compares the identity a hub shows with the one pinned for its origin. */
export function checkPin(origin: string, identity: HubIdentity): PinCheck {
  const pin = pinFor(origin);
  if (!pin) return { kind: 'first-use' };
  if (identity.fingerprint === pin.fingerprint) return { kind: 'match', pin };
  return { kind: 'mismatch', pin, received: identity.fingerprint };
}

/** The first 16 characters of a fingerprint, for display. */
export function shortFingerprint(fingerprint: string | null): string {
  return fingerprint ? `${fingerprint.slice(0, 16)}…` : 'no certificate';
}
