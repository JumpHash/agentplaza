// The git platforms' client: the owner's authorizations of the plaza on GitHub, GitLab and Forgejo, the repositories
// they may use, and the connections of those repositories to projects. Every call goes through the hub's owner door
// under `/v1/platforms` (crates/apid/src/platforms/mod.rs); the types follow the hub's real answers. Tokens and client
// secrets are sent once and never come back.

import { call } from './api';

export type PlatformKind = 'github' | 'gitlab' | 'forgejo';
/** How an authorization was given: the plaza's GitHub App, a sign-in on the platform, or a token the owner pasted. */
export type PlatformVia = 'app' | 'oauth' | 'token';
/** What an authorization covers: the owner's account, an organisation (GitLab: group), or one repository. */
export type PlatformLevel = 'user' | 'org' | 'repo';

export interface PlatformKindInfo { kind: PlatformKind; label: string; default_host: string }

export interface GitHubApp { id: number; slug: string; name: string; owner: string; html_url: string }

/** An app the plaza is registered as on a host: the plaza's GitHub App (`app` set), or an OAuth app the owner made. */
export interface PlatformRegistration { id: string; kind: PlatformKind; host: string; client_id: string; app: GitHubApp | null; created_ms: number }

/** A person as the platform knows them: the owner, or the user or organisation an installation is on. */
export interface PlatformAccount { id: number; login: string; name: string; email: string; avatar_url: string }

/** What the plaza may use in one namespace: every repository of it, or the ones named. */
export interface NamespaceGrant { all: boolean; repos: string[] }

export interface PlatformIntegration {
  id: string; kind: PlatformKind; host: string; via: PlatformVia; level: PlatformLevel;
  registration: string | null; installation: number | null;
  /** An installation's choice on GitHub: `all` or `selected`. */
  repository_selection: string | null;
  /** Where the owner changes the installation on the platform. */
  manage_url: string | null;
  /** The organisation, group or repository an organisation- or repository-level authorization is for. */
  target: string | null;
  account: PlatformAccount;
  scopes: string[];
  expires_ms: number | null;
  grants: { namespaces: Record<string, NamespaceGrant> };
  created_ms: number;
  checked_ms: number;
  /** Set when the authorization needs the owner (uninstalled on GitHub, an expired sign-in). */
  error: string | null;
  label: string;
  /** The account exports are authored by on that host. */
  identity: PlatformAccount;
}

export interface PlatformList {
  kinds: PlatformKindInfo[];
  registrations: PlatformRegistration[];
  integrations: PlatformIntegration[];
  /** The URL the platform sends the browser back to; the owner registers it on a GitLab or Forgejo app. */
  callback: string;
}

export interface PlatformNamespace { name: string; kind: 'user' | 'org' | 'group'; display: string; id: number; grant: NamespaceGrant | null }

/** A repository's connection to a project: the project's Weft repo, the connection's id and its state. */
export interface PlatformConnection { weft_repo: string; project: string | null; connection: string; state: string }

export interface PlatformRepo {
  full_name: string; namespace: string; name: string; private: boolean; default_branch: string;
  clone_url: string; web_url: string; can_push: boolean; granted: boolean; connections: PlatformConnection[];
}

/** A started flow: open `go` in the browser, then poll `state`. */
export interface FlowStart { state: string; go: string }

export interface FlowStatus {
  status: 'pending' | 'done' | 'failed' | 'expired';
  detail?: { integrations?: string[]; [key: string]: unknown };
  error?: string;
}

/** What the first import of a connected repository did: changes and releases, or that it is empty, or why it failed. */
export interface PlatformImport { commits?: number; releases?: string[]; empty?: boolean; error?: string }

export interface ConnectResult {
  weft_repo: string; connection: string; remote: string; branch: string; author: string;
  imported?: PlatformImport;
  signing_key?: 'registered' | { error: string };
}

export interface PlatformProject { id: string; title: string; archived?: boolean }

export interface GrantsBody { namespaces: Record<string, NamespaceGrant> }

export interface RegisterBody { kind: PlatformKind; host?: string; client_id: string; client_secret: string }

export interface PlatformTokenBody { kind: PlatformKind; host?: string; token: string; level?: PlatformLevel; target?: string }

const BASE = '/v1/platforms';
const seg = (text: string): string => encodeURIComponent(text);

export const platforms = {
  /** The kinds, the registered apps, the authorizations and the callback URL. */
  list: () => call<PlatformList>('GET', BASE),

  /** Creates the plaza's GitHub App on a host (owned by `org` when given) and gives the page to open. */
  createGitHubApp: (body: { host?: string; org?: string; name?: string }) => call<FlowStart>('POST', `${BASE}/github/app`, body),
  /** Installs the plaza's GitHub App on a user or an organisation; GitHub shows its own repository picker. */
  installGitHub: (registration: string) => call<FlowStart>('POST', `${BASE}/github/install`, { registration }),
  /** Records an OAuth app the owner registered on a host. */
  register: (body: RegisterBody) => call<PlatformRegistration>('POST', `${BASE}/registrations`, body),
  /** Forgets a registered app and its secret. */
  forgetRegistration: (id: string) => call<{ removed: string }>('DELETE', `${BASE}/registrations/${seg(id)}`),
  /** The owner's sign-in through a registered app. */
  authorize: (registration: string) => call<FlowStart>('POST', `${BASE}/authorize`, { registration }),
  /** An authorization by token, checked with the platform. */
  byToken: (body: PlatformTokenBody) => call<PlatformIntegration>('POST', `${BASE}/tokens`, body),
  /** How a started flow went: pending, done, failed or expired. */
  flow: (state: string) => call<FlowStatus>('GET', `${BASE}/flows/${seg(state)}`),

  /** Reads the account (and an app's installations) again. */
  sync: (id: string) => call<PlatformIntegration>('POST', `${BASE}/${seg(id)}/sync`),
  /** Forgets an authorization and its tokens. */
  remove: (id: string) => call<{ removed: string }>('DELETE', `${BASE}/${seg(id)}`),
  /** The namespaces an authorization reaches, each with the owner's grant. */
  namespaces: (id: string) => call<PlatformNamespace[]>('GET', `${BASE}/${seg(id)}/namespaces`),
  /** One namespace's repositories, granted or not, and where each is connected. */
  repos: (id: string, namespace: string) => call<PlatformRepo[]>('GET', `${BASE}/${seg(id)}/repos?namespace=${encodeURIComponent(namespace)}`),
  /** The owner's choice: the whole map of namespaces is replaced. */
  saveGrants: (id: string, body: GrantsBody) => call<PlatformIntegration>('PUT', `${BASE}/${seg(id)}/grants`, body),
  /** A granted repository joins a project. */
  connect: (id: string, body: { repo: string; project: string; sign?: boolean; export?: boolean }) => call<ConnectResult>('POST', `${BASE}/${seg(id)}/connect`, body),
  /** The project's repo stops exporting to the repository. */
  disconnect: (id: string, body: { weft_repo: string; connection: string }) =>
    call<{ weft_repo: string; connection: string; removed: boolean }>('POST', `${BASE}/${seg(id)}/disconnect`, body),

  /** The projects a repository can join. */
  projects: () => call<PlatformProject[]>('GET', '/v1/projects'),
};
