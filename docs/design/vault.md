# The vault: every secret the hub holds, encrypted, scoped and audited

The hub makes every outside call itself: model providers, connectors, Hugging Face, MCP servers. Agents never
hold a key; they ask the hub, and the hub decides. The vault is where those keys live. It replaces
`secrets.json`, `connector-secrets.json`, the Hugging Face token file and the `--env-file` keys, which were
plaintext on disk.

## Goals

- **Encrypted at rest.** A copy of the state directory, a Postgres dump or a backup holds only ciphertext.
  The key that opens it is not stored next to it.
- **Write-only.** No route, log, event, error, transcript, metric or panic message carries a secret's value.
  The console shows a name, a fingerprint and the last four characters, never more.
- **Scoped.** A secret is usable plaza-wide, or only by agents running on chosen nodes. The node is the
  one the hub assigned (an agent lives on the node whose CID range holds its CID). A guest cannot claim it.
- **Bound.** A secret names the consumers that may read it (`provider:anthropic`, `mcp:github`, `hf`, …).
  An MCP server configured with someone else's key name gets nothing.
- **Tamper-evident.** Every ciphertext is bound to its id, version, name, scope and bindings. The metadata
  carries a MAC. The audit log is a MAC chain. Any edit made outside the API is detected on load.
- **Recoverable.** Several unseal methods (keyslots) open the same root key: a key file for unattended
  starts, a passphrase, and a one-time recovery code.

Out of scope: protecting against code running as the hub's own user while the vault is unsealed. That code
can read the hub's memory. Agents run in microVMs and never on the host.

## Key hierarchy

```
keyslot (keyfile | passphrase | recovery) ──unwraps──▶ root key RK (32 random bytes)
RK ──HKDF-SHA256(salt = vault id, info = …)──▶ K_wrap   "apz-vault/v1/wrap"    wraps every data key
                                             ▶ K_index  "apz-vault/v1/index"   MACs the metadata, fingerprints, seals hints
                                             ▶ K_audit  "apz-vault/v1/audit"   MACs the audit chain
secret version ──▶ its own data key DEK (32 random bytes)
```

- **Cipher.** XChaCha20-Poly1305 everywhere, with a 24-byte random nonce per encryption (`chacha20poly1305`).
- **A value.** `ct = XChaCha20Poly1305(DEK, nonce, value, aad)`, where
  `aad = "apz-vault/v1/value\0" ‖ id ‖ "\0" ‖ version ‖ "\0" ‖ name ‖ "\0" ‖ canonical_json(scope) ‖ "\0" ‖ canonical_json(bindings)`.
  Moving a ciphertext to another entry, version, scope or binding set fails to decrypt.
  - Changing the scope or bindings re-encrypts the current version under the new AAD. The value never
    leaves the vault to do it.
- **A data key.** `dek_ct = XChaCha20Poly1305(K_wrap, nonce, DEK, "apz-vault/v1/dek\0" ‖ id ‖ "\0" ‖ version)`.
- **A keyslot.** It wraps RK with a key-encryption key (KEK), with AAD `"apz-vault/v1/slot\0" ‖ vault_id ‖ "\0" ‖ slot_id`:
  - `keyfile`: the KEK is 32 random bytes in the key file (see below).
  - `passphrase`: the KEK is Argon2id(passphrase, 16-byte salt, m = 256 MiB, t = 3, p = 1). The parameters
    are stored in the slot, so they can be raised later. The minimum passphrase is 12 characters.
  - `recovery`: the KEK is HKDF from a 32-byte recovery code. The code is shown once as 52 base32
    characters in 4-character groups. Making a new code replaces the old slot.
- **Fingerprint.** `fp = hex(HMAC-SHA256(K_index, value))[..8]`. It shows that two entries hold the same
  value, and that a rotation took, without revealing anything.
- **Hint.** The last 4 characters of a value of 20 or more; shorter values have none. It is sealed as
  `hint_ct = XChaCha20Poly1305(K_index, hint_nonce, last4, "apz-vault/v1/hint\0" ‖ id ‖ "\0" ‖ version)`.
  Views decrypt it while unsealed; a sealed vault shows none, and `status` never does. It keeps its own
  AAD (id and version only), so scope and binding changes and rollbacks leave it alone. A rekey re-seals
  it under the new K_index. A plaintext `hint` from an older file is moved into `hint_ct` on the next
  unsealed start, with one `migrate` audit line ("hints encrypted").
- **Rekey** (`POST /v1/vault/rekey`). A new RK, every DEK re-wrapped, every keyslot re-wrapped, and new
  index and audit keys. The audit chain records the rekey and restarts its MAC key from that entry. One
  atomic write.
- **Memory.** Plaintext lives in `zeroize::Zeroizing` buffers and values move as `secrecy::SecretString`.
  Nothing secret implements `Debug`, `Display` or `Serialize`. On Linux, apid sets
  `prctl(PR_SET_DUMPABLE, 0)` in release builds, so a crash leaves no core with keys in it. A sealed vault
  zeroizes RK and the derived keys.

### The key file

`apz_os::dirs::config()/vault.key` (or the path given by apid's `--vault-key FILE`; tests and temporary hubs
always pass a path in their own temp directory) holds one line, `apz-vault-keyfile/v1 <base64 32 bytes>`. It lives in
the config directory, never the state directory, so a copy of the state directory alone opens nothing.
- It is created with `O_EXCL` and mode 0600; on Windows its ACL grants only the current user.
- apid refuses a key file with group or world permissions.
- A vault without a keyfile slot starts **sealed** and waits for the owner: a passphrase or the recovery
  code in the console, or `apz vault unseal`.

## Storage

- **Where.** A Postgres component `vault` when Postgres is configured, else `state_dir/vault.json` (0600).
  The plaza's usual persistence rule applies.
- **How writes land.** Writes are atomic: temp file, `fsync`, rename, directory `fsync`. Postgres is
  written in one statement.
- **Document shape:**
  `{format:1, vault_id, created_ms, slots:[…], entries:[…], tombstones:[…], index_mac}`.
- **What `index_mac` covers.** It is `HMAC(K_index, canonical_json(slots without their wrapped keys,
  entries without usage counters, tombstones))`. Load verifies it after unseal. On a mismatch the vault's
  state is `tampered`: nothing resolves, and the owner sees what failed.
- **Usage counters.** `last_used_ms` and `use_count` change on every use, so they sit outside the MAC and
  are saved by the same 3 s saver the plaza uses for its other counters.
- **Versions.** Each entry keeps its current version and the two before it, for rollback. Older versions
  are dropped and their bytes overwritten in memory.

## Entries, scopes and bindings

```
Entry {
  id: "sec_<12 base32>",
  name: "ANTHROPIC_API_KEY",      // [A-Z][A-Z0-9_]{1,63}
  kind: "api_key" | "token" | "password" | "generic",
  label, created_ms, updated_ms, created_by,
  scope: {kind: "plaza"} | {kind: "nodes", nodes: ["n0", "n3"]},
  bindings: ["provider:anthropic", "mcp:github", "connector:slack", "hf"],
  current: 3,
  versions: [{v, created_ms, nonce, ct, dek_nonce, dek_ct, fp, hint_nonce, hint_ct}],
  last_used_ms, use_count,
}
```

- **Several entries per name.** One name can have several entries with different scopes. That is how a
  node gets its own Anthropic key while the rest of the plaza shares another.
  - At most one entry per name has scope `plaza`.
  - The node sets of a name's `nodes` entries are disjoint. A create or patch that breaks either rule
    gets a 409.
- **Resolution: `resolve(name, consumer)`.** The consumer is `{binding, on_behalf}`, where `on_behalf` is
  `Agent{cid}`, `Owner{node?}` or `Hub`.
  1. If the vault is sealed or tampered, fail with `VaultUnavailable` (HTTP 503 `vault sealed` on routes
     that need it).
  2. Find the node:
     - for an agent, the node whose CID range holds its CID (`fleet`/`nodes`), never anything the guest
       sent;
     - for the owner, the node they chose for a test (default `n0`);
     - for the hub's own background work, no node.
  3. Candidates are the entries named `name` whose bindings contain `consumer.binding`. Entries without
     that binding are invisible to this consumer.
  4. Prefer the `nodes` entry that contains the node, else the `plaza` entry, else `NotFound` (or
     `OutOfScope` when a `nodes` entry exists for other nodes only).
  5. Decrypt the current version and return `SecretString`. Count the use.
  6. Every denial is audited. Successful uses are audited aggregated: one entry per
     (entry, consumer, node) per minute.
- **Binding names:**
  - `provider:<provider id>`. A Bedrock-kind provider's AWS pair (`AWS_ACCESS_KEY_ID`,
    `AWS_SECRET_ACCESS_KEY`, optional `AWS_SESSION_TOKEN`) is bound to its own `provider:<id>`, so each
    Bedrock provider has its own pair and node scopes work per provider. Set it with
    `apz vault set NAME --bind provider:<id>` (the preset's id is `bedrock`). Renaming or removing a
    provider does not carry its pair over; the view then reports `missing` for the new id.
    The view's `credential` for such a provider is `secret` when both required keys resolve on this
    machine, else the vault's reason: `missing`, `out_of_scope` or `sealed`.
  - `connector:<connector id>`
  - `mcp:<server id>`
  - `hf` (the Hugging Face token)
  - `tool:<tool id>` (host tools that call out, e.g. web search)
  The vault lists every binding the hub can use (`GET /v1/vault/consumers`), with the secret name each
  one wants and what it resolves to on each node.

## Audit

- **Where.** `state_dir/vault-audit.jsonl` (0600, append-only), mirrored to a Postgres table when
  configured.
- **Line shape.** `{seq, ts_ms, event, entry?, name?, actor, binding?, node?, cid?, detail?, prev, mac}`,
  with `mac = hex(HMAC(K_audit, prev ‖ canonical_json(line without mac)))`.
- **Events:**
  - vault life: `init`, `unseal`, `unseal_failed`, `seal`, `slot_added`, `slot_removed`, `recovery_made`,
    `rekey`;
  - entries: `create`, `rotate`, `rollback`, `scope`, `bindings`, `delete`;
  - imports: `import`, `migrate`;
  - uses: `use` (aggregated), `deny`;
  - integrity: `tamper`.
- **Verification.** It runs on unseal and on demand (`GET /v1/vault/audit?verify=1`). A broken chain is
  shown on the Vault screen and the event stream (`vault.audit_broken`). It does not seal the vault:
  integrity of the secrets is guarded by the index MAC and the AEAD.
- **Rate limits.** Failed unseals are limited to 5 per minute, then a 10-minute lockout. Every failure
  is audited.

## Routes (owner auth; never reachable from an agent's gateway)

```
GET    /v1/vault                       → {state: uninitialized|sealed|unsealed|tampered, slots:[{id,kind,created_ms}],
                                          entries, nodes:[{id,name}], audit_ok, rekeyed_ms}
POST   /v1/vault/unseal   {passphrase | recovery_code} → the status
POST   /v1/vault/seal                  → the status
POST   /v1/vault/slots    {kind:"passphrase", passphrase} | {kind:"keyfile"} → the status
DELETE /v1/vault/slots/{id}            → the status (409 when it is the last slot)
POST   /v1/vault/recovery              → {recovery_code} (shown once; replaces the old recovery slot)
POST   /v1/vault/rekey                 → the status
GET    /v1/vault/secrets               → [EntryView]   (no values; fp and hint per version)
POST   /v1/vault/secrets  {name, kind, label?, value, scope, bindings} → EntryView
PUT    /v1/vault/secrets/{id}/value    {value} → EntryView (a new version)
PATCH  /v1/vault/secrets/{id}          {label?, scope?, bindings?} → EntryView
POST   /v1/vault/secrets/{id}/rollback {v} → EntryView
DELETE /v1/vault/secrets/{id}          → 204 (adds a tombstone so env import never brings it back)
POST   /v1/vault/secrets/{id}/test     {node?} → {ok, detail}  (the bound consumer makes a harmless call)
POST   /v1/vault/import   {format:"dotenv", text, scope, bindings?:{NAME:[binding…]}}
                                       → {imported:[{name,id}], skipped:[{name,reason}]}
GET    /v1/vault/consumers             → [{binding, label, wants: name, per_node:{n0: "entry id"|"missing"|"out_of_scope"}}]
GET    /v1/vault/audit?after=&limit=&verify=1 → {entries, chain_ok}
```

- **No reveal route.** A request body that carries a value is never logged. `apz-http` logs no bodies, and
  the vault's handlers never echo input.
- **Live updates.** Changes are pushed on the plaza's WebSocket as `vault.changed`, carrying entry ids
  only.

## Migration and the env file

- **On the first unsealed start** the vault imports:
  - `secrets.json` (bindings from the providers that reference each name);
  - `connector-secrets.json` (`connector:<id>`);
  - the Hugging Face token (`hf`);
  - the env-file variables that a provider, connector or tool names.
  - An AWS pair (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`) found in
    `secrets.json`, the env file or the process environment is bound to every existing Bedrock-kind
    provider (`provider:<id>` each), or to `provider:bedrock` when none exists. A Bedrock provider added
    later does not inherit an imported pair: bind its pair with `apz vault set --bind provider:<id>`.
  - Each import is decrypted back and compared before the plaintext file is overwritten, `fsync`ed and
    deleted. Everything is audited as `migrate`.
- **The env file is deprecated.** At each start, a variable that is in the env file and not in the vault
  (and not tombstoned) is imported once (plaza scope, inferred bindings).
  - apid then logs: `keys in <file> were moved into the vault; delete the file`.
  - The console shows a banner while the file still holds keys.
  - The vault is the only source after import. The env file's value is ignored from then on.
- **The CLI** (`apz vault …`) reads values from stdin or a hidden prompt, never from argv:
  - `apz vault status | unseal | seal | list | audit | rekey | recovery`
  - `apz vault set NAME [--kind api_key] [--scope plaza | --nodes n1,n2] [--bind provider:anthropic …]`
  - `apz vault rm ID`
  - `apz vault import FILE.env [--nodes …] [--shred]` prints that .env files are discouraged; `--shred`
    overwrites and deletes the file after a verified import.

## Tests that must stay green

- Crypto:
  - a round trip works;
  - a flipped byte in a ciphertext, nonce, DEK or slot fails;
  - swapping ciphertexts between entries or versions fails;
  - a scope or binding edited in the file fails the index MAC (`tampered`).
- Slots:
  - a wrong passphrase fails and is rate-limited;
  - add and remove slots, and the last slot cannot be removed;
  - the recovery code opens the vault;
  - a key file with 0644 permissions is refused.
- Rekey keeps every value and changes every wrapped key.
- Resolution:
  - a node entry beats the plaza entry;
  - an agent on another node gets `OutOfScope`;
  - a missing binding is invisible;
  - the node comes from the CID range, not the request.
- 409 on a second plaza entry for a name, and on overlapping node sets.
- No value appears in any route response, event or log line. Each test writes a sentinel value and greps
  every output.
- The audit chain verifies, and an edited line breaks it at that line.
- Migration:
  - plaintext files are gone afterwards and the values resolve;
  - a deleted entry is not re-imported from the env file.
- Crash safety: a leftover temp file is ignored, and a half-written file never replaces a good one.
