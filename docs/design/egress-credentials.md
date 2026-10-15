# Egress credentials

Status (2026-10-15): built in `crates/apid/src/egress_inject.rs` (rules, the CA, the decisions, the routes), in
`crates/vsockd/src/egress_tls.rs` (the TLS interception on the node), and in `crates/agent-harness/src/trust.rs` (the
guest's trust in the CA).

An agent's outbound HTTPS request can carry a secret the agent never holds. The owner writes a rule: for these roles,
requests to this host get this header, with a vault secret in its value. The hub's egress proxy adds the header on the
way out. The secret never enters the VM, and the agent cannot read it, print it or send it somewhere else.

CubeSandbox's egress proxy (Apache-2.0, Copyright Tencent) does the same for every HTTP(S) request, through OpenResty.
Here only the hosts the owner names are opened. Everything else stays a tunnel the proxy does not read.

## Rules

A rule is written by the owner (`/v1/egress/inject`). It names:

| field | meaning |
|---|---|
| roles | The agent roles the rule applies to. Empty means every role. |
| host | An exact host, or `*.suffix` for any subdomain. |
| port | The port. Default 443. Port 80 is refused: a credential is only added to HTTPS. |
| methods | HTTP methods the rule covers. Empty means any. |
| path prefix | The rule covers paths under this prefix. |
| header | The header to set. A header that frames or routes the request (hop-by-hop headers and the like, 11 names in all) is refused. |
| format | The header's value, with `${SECRET}` where the secret's value goes. |
| secret | The name of a vault secret. |

The first rule that matches wins. A rule must name a vault secret that is bound to the `egress` consumer
(`BINDING` in `egress_inject.rs`). The vault screen lists each secret as wanted by the egress rules. The owner's
rules are owner-only, and a rule shows the secret's name, never its value.

## What is intercepted

Only a host that a rule could match, for an agent whose role the rule covers, is intercepted. The egress proxy on the
node asks the hub before it dials (`POST /internal/egress/check`), and the answer says `intercept` only in that case.
Every other connection stays an opaque CONNECT tunnel. The proxy splices bytes and does not read them, as it did before
rules existed.

## The CA and the leaves

The hub has one CA, named `AgentPlaza egress CA`, valid for about ten years. It is created on first use. Its
certificate (`egress-ca.pem`) and its key (`egress-ca.key`) sit in the hub's state directory (the key is mode 0600), and
the key never leaves the hub.

For each intercepted host the hub signs a leaf certificate with the CA. A leaf is valid for 7 days and is cached per
host until a day before it ends. It is signed with ECDSA P-256, has the host as its name, and is for server
authentication only. The node keeps the leaf it was given per (CID, host), until an hour before it expires.
`POST /internal/egress/cert` returns the leaf and its key for one host, to a node that asks for an agent that is
intercepted there. The key is the answer's only copy, and nothing logs it.

## How the guest trusts the CA

The guest's clients must accept the leaves, so they must trust the CA. The hub's profile for an agent carries the CA
(`AgentProfile.egress_ca`) only when a rule could inject for that agent's role. The harness's `trust::install` writes
it where the guest's clients look:

| where | what |
|---|---|
| `egress-ca.pem` | The CA alone, for clients that take one extra root. |
| `ca-bundle.pem` | The system bundle plus the CA, for clients that take a whole bundle. |
| the system trust store | So a client that reads the store directly trusts the CA too. |

The environment variables that point clients at these files: `SSL_CERT_FILE`, `CURL_CA_BUNDLE`,
`REQUESTS_CA_BUNDLE` (the bundle) and `NODE_EXTRA_CA_CERTS` (the CA alone). A CA is accepted only when it is one PEM
certificate block of at most 16 KiB. Anything else is refused, and the guest keeps the trust it had. A role whose rules
are all removed gets no CA, and the system store loses it on the next profile. A removed rule stops being trusted.

## Injection, per request

For an intercepted connection the node (`vsockd`, `egress_tls.rs`):

1. Terminates the guest's TLS with the leaf for the host.
2. Opens its own TLS connection to the real host, verified against the webpki roots, over the addresses the policy
   checked. ALPN is `http/1.1` on both sides.
3. Reads each request head the guest sends (up to 16 KiB), parses it in origin form, and asks the hub
   (`POST /internal/egress/inject {cid, host, port, method, path}`). The answer is `{header, value, rule}`, or `{}` when
   no rule applies. The header replaces the guest's own copy of that header. The head then goes upstream with its body,
   framed by `Content-Length`, or `chunked` relayed chunk by chunk through the trailers.
4. Relays the upstream's bytes to the guest, raw, as they arrive. An `Upgrade` request ends the parsing, and its bytes
   are copied raw from then on.

A request the node cannot frame, or whose injection the hub cannot decide, is not sent. The connection is closed with
a logged reason.

## Audit

Each injected request is a transcript entry for the agent: host, method, path and the rule's name. The value is never
written. No log line, transcript entry or audit record carries a header value or a key. The node's per-connection
record (the `egress` entries the relay reports for every connection) is the same as for an un-intercepted connection.

## Routes

| route | who | what |
|---|---|---|
| `GET /v1/egress/inject` | owner | Lists the rules. |
| `POST /v1/egress/inject` | owner | Creates a rule. |
| `PATCH /v1/egress/inject/{id}` | owner | Changes a rule. |
| `DELETE /v1/egress/inject/{id}` | owner | Deletes a rule. |
| `POST /internal/egress/check` | node | Whether to intercept a host for an agent, and the laws and fetch blocklist check. |
| `POST /internal/egress/cert` | node | `{cid, host}` → the leaf certificate and key for that host (403 when the host is not intercepted for the agent). |
| `POST /internal/egress/inject` | node | `{cid, host, port, method, path}` → `{header, value, rule}`, or `{}`. |

The `internal` routes take the node's internal token, as the other `/internal` routes do (see
[`architecture.md`](../architecture.md)).

## Limits

- A rule covers HTTPS on a named host only. It does not add headers to plain HTTP, and it does not inspect the body.
- A guest that does not trust the CA gets a TLS error for an intercepted host, and never a request without the header.
- The guest's network device does not exist. Egress reaches the guest only through the node's relay, as for every
  agent (see [`architecture.md`](../architecture.md), "The agent loop").

See also [`vm-snapshots.md`](vm-snapshots.md) and [`e2b.md`](e2b.md), which both use the same egress path.

## In the consoles

The Egress screen of the web console and the desktop app lists the rules, adds one (with the defaults above), enables or
disables it and deletes it; the CLI does the same with `agentplaza egress list|add|enable|disable|rm`.
