"""Protocol draft tests: round trips per stream, plus security properties of the schema itself.

Run: python -I tests/test_protocol.py <generated-python-dir>   (check.sh does this)
"""
import struct
import sys

sys.path.insert(0, sys.argv[1])
from google.protobuf import json_format
from google.protobuf.descriptor import FieldDescriptor
from agentplaza.v1 import (blob_pb2 as blob, bus_pb2 as bus, common_pb2 as c, daemon_pb2 as api,
                           fs_pb2 as fs, model_pb2 as model, node_pb2 as node, owner_pb2 as own,
                           session_pb2 as ss, stream_pb2 as st, tool_pb2 as tool)

failures = 0


def check(name, cond):
    global failures
    print(("ok    " if cond else "FAIL  ") + name)
    failures += 0 if cond else 1


def rt(msg):
    return type(msg).FromString(msg.SerializeToString())


def fields(msg_cls):
    return {f.name for f in msg_cls.DESCRIPTOR.fields}


def frame(msg):
    b = msg.SerializeToString()
    return struct.pack(">I", len(b)) + b


def unframe(buf, cls):
    n = struct.unpack(">I", buf[:4])[0]
    return cls.FromString(buf[4:4 + n]), buf[4 + n:]


# ---- framing and stream setup (0085) ----
buf = frame(st.StreamOpen(kind=st.STREAM_KIND_FS, session_token="t", stream_id=7)) + frame(
    fs.FsRequest(op_id=1, lookup=fs.FsLookup(path="/plaza/groups/g1/notes.md")))
o, rest = unframe(buf, st.StreamOpen)
r, rest = unframe(rest, fs.FsRequest)
check("framing: StreamOpen then FsRequest", o.kind == st.STREAM_KIND_FS and r.lookup.path.endswith("notes.md") and rest == b"")

# ---- security properties of the schema ----
check("BUS: Outgoing has no sender field (sender is stamped by the orchestrator)",
      not ({"from_agent", "from_name", "from_role", "sender"} & fields(bus.Outgoing)))
check("BUS: Outgoing has no taint field (taint is computed, never claimed)", "taint" not in fields(bus.Outgoing))
check("BUS: Envelope carries stamped sender and taint", {"from_agent", "taint"} <= fields(bus.Envelope))
check("BLOB: PutStart declares every chunk hash up front (verifiable)", "chunk_hashes" in fields(blob.BlobPutStart))
check("Session: harness can't pick its agent id (assigned in HelloAck)", "agent_id" not in fields(ss.Hello) and "agent_id" in fields(ss.HelloAck))
check("Relay: vm_id/node_id only in RelayHeader (added by the machine), not in StreamOpen",
      not ({"vm_id", "node_id"} & fields(st.StreamOpen)) and {"vm_id", "node_id"} <= fields(st.RelayHeader))
check("Tool: denials can name the law (Error.law_id)", "law_id" in fields(c.Error))
check("NODE: metric samples have no run, agent, session or trace id fields (low cardinality)",
      not ({"run_id", "agent_id", "session_id", "trace", "trace_id"} & fields(node.MetricSample)))
check("NODE: the orchestrator sets per-VM caps in HelloAck (RT-012)", "limits" in fields(node.NodeHelloAck)
      and {"max_streams_per_vm", "max_frame_bytes"} <= fields(node.NodeLimits))
check("NODE: tripwire evidence can be uploaded in full (BlobRef)", "evidence_blob" in fields(node.TripwireFired))

# ---- NODE metrics, logs and caps (0114, 0122) ----
m = rt(node.NodeUp(metrics=node.NodeMetrics(monotonic_ms=5, samples=[node.MetricSample(
    name="apz_vmm_exits_total", kind=node.MetricSample.KIND_COUNTER, value=12.0, labels={"trust_class": "standard"})])))
check("NODE: metrics round trip with labels", m.metrics.samples[0].labels["trust_class"] == "standard" and m.metrics.samples[0].value == 12.0)
lg = rt(node.NodeUp(logs=node.LogBatch(records=[node.LogRecord(level=node.LogRecord.LEVEL_ERROR, component="vmm", msg="exit", vm_id="vm1")], dropped=3)))
check("NODE: log batch round trip, reports drops", lg.logs.dropped == 3 and lg.logs.records[0].component == "vmm")
cap = rt(node.NodeEvent(cap_hit=node.StreamCapHit(vm_id="vm1", cap="streams", value=65)))
check("NODE: stream cap hit is reported", cap.cap_hit.cap == "streams" and cap.cap_hit.value == 65)
ack = rt(node.NodeDown(hello_ack=node.NodeHelloAck(limits=node.NodeLimits(max_streams_per_vm=64, max_frame_bytes=4 << 20))))
check("NODE: limits arrive with HelloAck", ack.hello_ack.limits.max_streams_per_vm == 64)

# ---- restore merge (0135) ----
check("Seal: decisions, model turns and acks carry the orchestrator's seal",
      "seal" in fields(tool.ToolDecision) and "seal" in fields(model.Completed) and "seal" in fields(ss.Ack))
check("Seal: the seal chains entries (prev_hash) and is signed", {"prev_hash", "entry_hash", "signature", "session_epoch"} <= fields(c.EntrySeal))
hello = rt(node.NodeUp(hello=node.NodeHello(node_id="m1", tails=[node.JournalTail(run_id="r1", ran_call_ids=["c7"], entries=[
    node.SealedEntry(seal=c.EntrySeal(run_id="r1", seq=41, prev_hash=b"p" * 32, entry_hash=b"e" * 32, signature=b"s" * 64), entry=b"x")])])))
check("NODE: hello offers a sealed journal tail after a restore", hello.hello.tails[0].entries[0].seal.seq == 41 and hello.hello.tails[0].ran_call_ids == ["c7"])
mr = rt(node.NodeDown(hello_ack=node.NodeHelloAck(merged=[node.MergeResult(run_id="r1", accepted_through_seq=48, rejected_from_seq=49, reason="bad_signature", cancelled_call_ids=["c9"])])))
check("NODE: the orchestrator reports how far it merged and what it cancelled", mr.hello_ack.merged[0].reason == "bad_signature" and mr.hello_ack.merged[0].cancelled_call_ids == ["c9"])

# ---- FS (/plaza) ----
w = rt(fs.FsResponse(op_id=9, released=fs.FsReleased(new_version=4, superseded=True)))
check("FS: release reports a superseded concurrent write", w.released.superseded and w.released.new_version == 4)
d = rt(fs.FsResponse(op_id=3, errno=fs.FS_EROFS, error=c.Error(code=c.Error.CODE_DENIED, law_id="")))
check("FS: write into artifacts maps to EROFS", d.errno == fs.FS_EROFS)
p = rt(fs.FsDown(push=fs.FsPush(invalidate=fs.FsInvalidate(path="/plaza/groups/g1/a.txt", version=5))))
check("FS: orchestrator can push cache invalidations", p.WhichOneof("msg") == "push" and p.push.invalidate.version == 5)
pub = rt(fs.FsRequest(op_id=4, plaza=fs.PlazaCommand(publish=fs.PlazaPublish(path="/workspace/out", name="report"))))
check("FS: plaza publish travels on the FS stream", pub.plaza.publish.name == "report")

# ---- BUS ----
up = rt(bus.BusUp(send=bus.Outgoing(client_id="c1", to=bus.Address(group="g1"), kind=bus.MESSAGE_KIND_REQUEST,
                                    body=c.Payload(inline=b"can you review PR 3?"))))
check("BUS: send to a group", up.send.to.WhichOneof("to") == "group")
env = rt(bus.BusDown(deliver=bus.Envelope(id="01J", from_agent="01A", from_role="builder", kind=bus.MESSAGE_KIND_REQUEST,
                                         taint=c.TAINT_UNTRUSTED, thread_turn=3)))
check("BUS: delivered envelope keeps stamped taint", env.deliver.taint == c.TAINT_UNTRUSTED)
rej = rt(bus.BusDown(rejected=bus.Rejected(client_id="c2", reason=bus.Rejected.REASON_PING_PONG)))
check("BUS: ping-pong rejection", rej.rejected.reason == bus.Rejected.REASON_PING_PONG)
claims = rt(bus.BusDown(claims=bus.ClaimsSnapshot(group_id="g1", claims=[bus.Claim(claim_id="k1", owner_agent="01A",
            state=bus.ClaimUpdate(status=bus.ClaimUpdate.STATUS_BLOCKED, blocker="needs API key scope"))])))
check("BUS: claims board snapshot", claims.claims.claims[0].state.status == bus.ClaimUpdate.STATUS_BLOCKED)

# ---- BLOB (resumable) ----
start = blob.BlobUp(start=blob.BlobPutStart(blob=c.BlobRef(blake3=b"\x01" * 32, size=600 * 1024),
                                            chunk_hashes=[b"\x02" * 32, b"\x03" * 32, b"\x04" * 32]))
need = rt(blob.BlobDown(need=blob.BlobNeed(missing_chunks=[2])))
check("BLOB: 600 KiB = 3 chunks; resumed upload asks only for the missing one",
      len(rt(start).start.chunk_hashes) == 3 and list(need.need.missing_chunks) == [2])

# ---- Node control and fencing (0102) ----
hello = rt(node.NodeUp(hello=node.NodeHello(node_id="pi5-garage", facts=node.NodeFacts(arch="aarch64", backend="kvm",
           irqchip="gicv2", host_page_size=16384), vms=[node.VmStatus(vm_id="vm-7", state=node.VmStatus.STATE_RUNNING)])))
ack = rt(node.NodeDown(hello_ack=node.NodeHelloAck(discard_vm_ids=["vm-7"])))
check("NODE: Pi 5 facts (GICv2, 16K pages) round-trip", hello.hello.facts.host_page_size == 16384)
check("NODE: returning machine is told to discard a moved VM", list(ack.hello_ack.discard_vm_ids) == ["vm-7"])
tw = rt(node.NodeUp(event=node.NodeEvent(tripwire=node.TripwireFired(vm_id="vm-3", rule="egress_denial_storm",
        action="paused", while_offline=True))))
check("NODE: tripwire fired offline is reported later", tw.event.tripwire.while_offline)

# ---- earlier streams still fine ----
mev = rt(model.ModelEvent(completed=model.Completed(seq=c.Seq(value=43), stop_reason="tool_use")))
td = rt(ss.ControlDown(tool_decision=tool.ToolDecision(call_id="c", verdict=tool.ToolDecision.VERDICT_PENDING_APPROVAL)))
mv = rt(api.MoveRunRequest(run_id="r1", from_transcript=True))
check("MODEL/CONTROL/API round-trips", mev.completed.seq.value == 43 and td.tool_decision.verdict == 3 and mv.from_transcript)

# ---- 0116: idempotency, tracing, capabilities, replay protection ----
IDEMPOTENCY_FIELDS = {"idempotency_key", "client_id", "request_id", "call_id", "command_id", "blob"}
MUTATING = [ (api.StartRunRequest, "start a run"), (api.MoveRunRequest, "move a run"),
             (api.SendMessageRequest, "owner message"), (model.ModelRequest, "model call"),
             (tool.ToolRequest, "tool request"), (bus.Outgoing, "agent message"), (bus.GroupPropose, "group proposal"),
             (node.NodeCommand, "machine command"), (fs.FsRequest, "file operation"), (blob.BlobPutStart, "blob upload") ]
missing = [n for cls, n in MUTATING if not (IDEMPOTENCY_FIELDS & fields(cls))]
check(f"Idempotency: all {len(MUTATING)} mutating messages carry a key" + (f" (missing: {missing})" if missing else ""), not missing)

TRACED = [st.StreamOpen, model.ModelRequest, tool.ToolRequest, bus.Outgoing, node.NodeCommand]
check("Tracing: stream setup and work-starting requests carry TraceContext",
      all("trace" in fields(c_) for c_ in TRACED) and {"trace_id", "span_id"} <= fields(c.TraceContext))

check("Capabilities: announced in every handshake (harness, orchestrator, machine)",
      all("capabilities" in fields(c_) for c_ in (ss.Hello, ss.HelloAck, node.NodeHello, node.NodeHelloAck)))
check("Capabilities: clients can ask the orchestrator (GetInfo RPC)",
      "GetInfo" in [m.name for m in api.DESCRIPTOR.services_by_name["AgentPlaza"].methods])

def negotiate(a, b):
    ma, mb = int(a.proto_version.split(".")[1]), int(b.proto_version.split(".")[1])
    if abs(ma - mb) > 1: return None                      # only current and previous minor (docs/design/protocol.md (not in the repo))
    return min(ma, mb), sorted(set(a.features) & set(b.features))
h = c.Capabilities(proto_version="1.4", features=["fs.v1", "bus.v1", "token.rotate"])
o = c.Capabilities(proto_version="1.5", features=["fs.v1", "blob.resume", "token.rotate"])
check("Capabilities: negotiation picks the lower minor and the shared features",
      negotiate(h, o) == (4, ["fs.v1", "token.rotate"]))
check("Capabilities: two minors apart is refused", negotiate(c.Capabilities(proto_version="1.2"), o) is None)

rot = rt(ss.ControlDown(token_rotate=ss.TokenRotate(session_token="t2", token_epoch=2, previous_valid_ms=30000)))
check("Replay: session tokens rotate with an epoch and a grace window",
      rot.token_rotate.token_epoch == 2 and "token_epoch" in fields(st.StreamOpen))
check("Replay: approval decisions echo a one-time nonce and expire",
      {"nonce", "expires_unix_ms"} <= fields(api.DecideApprovalRequest) and "nonce" in fields(api.Approval))
check("Priority: four scheduling classes, control highest",
      [st.Priority.Name(v) for v in sorted(st.Priority.values())][1] == "PRIORITY_CONTROL" and len(st.Priority.values()) == 5)

# ---- owner API, M2/M3 (owner.proto: 0047, 0067, 0094, 0124, 0127-0135) ----
OWNER_SERVICES = {"Inbox", "Towns", "Tasks", "Agents", "Machines", "Treasury", "Agora", "Codex", "Security",
                  "Vault", "Restore", "Settings", "Tribunal"}
services = own.DESCRIPTOR.services_by_name
methods = [(s.name, m) for s in services.values() for m in s.methods]
check(f"Owner API: one service per console section ({len(services)} services, {len(methods)} RPCs)",
      set(services) == OWNER_SERVICES)


def all_messages(file_desc):
    def walk(d):
        yield d
        for n in d.nested_types:
            yield from walk(n)
    for d in file_desc.message_types_by_name.values():
        yield from walk(d)


def reachable(desc, seen=None):
    """Every message type reachable from desc through its fields (across files)."""
    seen = set() if seen is None else seen
    if desc.full_name in seen:
        return seen
    seen.add(desc.full_name)
    for f in desc.fields:
        if f.message_type is not None:
            reachable(f.message_type, seen)
    return seen


def by_name(full_name):
    return own.DESCRIPTOR.pool.FindMessageTypeByName(full_name)


# (a) idempotency: every RPC that changes state takes a key. Reads are List*/Get*/Verify*; Begin* only issues
# a fresh one-time challenge, so a retry is harmless (it gets another).
READ_PREFIXES, CHALLENGE_PREFIXES = ("List", "Get", "Verify"), ("Begin",)
mutating = [(s, m) for s, m in methods if not m.name.startswith(READ_PREFIXES + CHALLENGE_PREFIXES)]
no_key = [f"{s}.{m.name}" for s, m in mutating if "idempotency_key" not in m.input_type.fields_by_name]
check(f"Owner API idempotency: all {len(mutating)} mutating RPCs carry idempotency_key"
      + (f" (missing: {no_key})" if no_key else ""), not no_key)
reads_with_key = [m.name for s, m in methods if m.name.startswith(READ_PREFIXES)
                  and "idempotency_key" in {f.name for f in m.input_type.fields}]
check("Owner API idempotency: read RPCs don't pretend to mutate (no key on List/Get/Verify)", not reads_with_key)

# (b) step-up: exactly these RPCs take a passkey assertion (0047, 0067, 0127, 0128, 0131; additions in daemon-api.md)
STEP_UP = {
    "Towns.RatifyTownChange", "Towns.ArchiveTown",                     # ratification (0067, 0127)
    "Agents.RetireAgent",                                             # irreversible
    "Machines.CreateMachineEnrollmentCode", "Settings.CreateDeviceEnrollmentCode",   # enrollment (0047)
    "Treasury.MoveBudget",                                            # 0128
    "Codex.SignProposal", "Codex.VetoBylaw",                          # law changes (0047, 0049, 0134)
    "Vault.UnlockVault", "Vault.PutCredential", "Vault.ListCredentials",  # 0128, 0131
    "Restore.ReviewWindow",                                           # recorded effects are irreversible (0135)
    "Settings.UnlockSession", "Settings.SetDeviceIdleLock", "Settings.RevokeDevice",  # 0128
    "Security.ReleaseVm",                                             # resuming a tripwire-stopped VM (0137)
    "Tribunal.DecideAppeal", "Tribunal.Pardon",                       # 0138
}
with_step_up = {f"{s}.{m.name}" for s, m in methods if "step_up" in m.input_type.fields_by_name}
check(f"Owner API step-up: exactly the {len(STEP_UP)} listed RPCs take step_up"
      + (f" (extra: {sorted(with_step_up - STEP_UP)}, missing: {sorted(STEP_UP - with_step_up)})"
         if with_step_up != STEP_UP else ""), with_step_up == STEP_UP)
check("Owner API step-up: every step_up field is a StepUp (nonce + assertion)",
      all(m.input_type.fields_by_name["step_up"].message_type.full_name == "agentplaza.v1.StepUp"
          for s, m in methods if f"{s}.{m.name}" in with_step_up)
      and {"nonce", "assertion"} <= fields(c.StepUp))
check("Owner API step-up: challenges are bound to the RPC and a digest of the request",
      {"rpc", "request_digest"} <= fields(own.BeginStepUpRequest) and "nonce" in fields(own.StepUpChallenge))

# (c) no field anywhere in owner.proto is named like a secret
BANNED = ("secret", "password", "passphrase", "plaintext", "cleartext", "api_key", "apikey", "private_key",
          "token_value", "session_token", "bearer")
bad = [f"{d.name}.{f.name}" for d in all_messages(own.DESCRIPTOR) for f in d.fields if any(b in f.name for b in BANNED)]
check("Owner API: no field is named like a secret value" + (f" ({bad})" if bad else ""), not bad)

# (d) listing credentials returns names and status, never a value; no bytes at all
ci = own.CredentialInfo.DESCRIPTOR
check("Vault: ListCredentials items carry name and status but no value",
      {"name", "status"} <= fields(own.CredentialInfo)
      and not ({"value", "ciphertext", "hpke_enc", "data", "key"} & fields(own.CredentialInfo))
      and own.ListCredentialsResponse.DESCRIPTOR.fields_by_name["credentials"].message_type is ci)
check("Vault: CredentialInfo has no bytes field (nothing can carry key material)",
      all(f.type != FieldDescriptor.TYPE_BYTES for f in ci.fields))

# (e) a new credential arrives only as ciphertext sealed on the owner's device
put = own.PutCredentialRequest.DESCRIPTOR
put_bytes = {f.name for f in put.fields if f.type == FieldDescriptor.TYPE_BYTES}
check("Vault: PutCredential carries only HPKE ciphertext (bytes fields = hpke_enc, ciphertext)",
      put_bytes == {"hpke_enc", "ciphertext"} and "vault_key_id" in fields(own.PutCredentialRequest)
      and not ({"value", "credential", "plaintext"} & fields(own.PutCredentialRequest)))
check("Vault: the unlock key is sealed to a one-time vault key, and vault keys are endorsed",
      {"hpke_enc", "sealed_unwrap_key", "nonce"} <= fields(own.UnlockVaultRequest)
      and "endorsement" in fields(own.VaultPublicKey) and "unlock_key" in fields(own.VaultUnlockChallenge))
SEALED = {"ciphertext", "hpke_enc", "sealed_unwrap_key"}
leaky = sorted({f"{s}.{m.name}" for s, m in methods for name in reachable(m.output_type)
                if SEALED & {f.name for f in by_name(name).fields}})
check("Vault: no RPC response (transitively) carries sealed secret material" + (f" ({leaky})" if leaky else ""), not leaky)

# pagination and orchestrator-written summaries
unpaged = [f"{s}.{m.name}" for s, m in methods if m.name.startswith("List")
           and not ({"page_size", "page_token"} <= {f.name for f in m.input_type.fields}
                    and "next_page_token" in {f.name for f in m.output_type.fields})]
check("Owner API: every List RPC pages (page_size, page_token, next_page_token)" + (f" ({unpaged})" if unpaged else ""),
      not unpaged)
summary_in = sorted({f"{s}.{m.name}" for s, m in methods for name in reachable(m.input_type)
                     if "summary" in {f.name for f in by_name(name).fields}})
check("Owner API: no request can set a summary (summaries are orchestrator-written, 0094)"
      + (f" ({summary_in})" if summary_in else ""), not summary_in)

# Needs you (0127)
item = rt(own.NeedsYouItem(item_id="i1", kind=own.NEEDS_YOU_KIND_APPROVAL, summary="Deploy shop-api v2 to production",
          deadline_unix_ms=1_760_000_000_000, on_expiry=own.NeedsYouItem.ON_EXPIRY_DENIED, law_id="C-004a",
          needs_step_up=True, act_with="agentplaza.v1.AgentPlaza/DecideApproval",
          approval=api.Approval(approval_id="a1", chain=["owner"], nonce=b"n" * 16)))
check("Inbox: an owner-step approval round-trips with deadline, expiry rule and law",
      item.WhichOneof("subject") == "approval" and list(item.approval.chain) == ["owner"]
      and item.on_expiry == own.NeedsYouItem.ON_EXPIRY_DENIED and item.law_id == "C-004a")
veto = own.NeedsYouItem(kind=own.NEEDS_YOU_KIND_BYLAW_VETO, on_expiry=own.NeedsYouItem.ON_EXPIRY_TAKES_EFFECT,
                        proposal=own.CodexProposal(proposal_id="p9", severity="low", veto_until_unix_ms=5))
js = json_format.MessageToJson(veto)
check("Inbox: items survive the JSON mapping the browser uses (0100)",
      json_format.Parse(js, own.NeedsYouItem()) == veto and "ON_EXPIRY_TAKES_EFFECT" in js)

# Towns (0124)
tc = rt(own.TownChange(change_id="tc1", town_id="shop", state=own.TownChange.STATE_AWAITING_RATIFICATION,
        widenings=[own.Widening(kind=own.Widening.KIND_EXTRA_HOST, path="/egress/extra_hosts/0", to="pypi.org"),
                   own.Widening(kind=own.Widening.KIND_BUDGET, path="/budget/monthly_credits", **{"from": "50000", "to": "80000"})]))
check("Towns: a config change lists its widenings over the template",
      [w.kind for w in tc.widenings] == [own.Widening.KIND_EXTRA_HOST, own.Widening.KIND_BUDGET]
      and getattr(tc.widenings[1], "from") == "50000")
check("Towns: town states follow the lifecycle (proposed, active, paused, archived, rejected)",
      {own.TownState.Name(v) for v in own.TownState.values()} >= {"TOWN_STATE_PROPOSED", "TOWN_STATE_ACTIVE",
       "TOWN_STATE_PAUSED", "TOWN_STATE_ARCHIVED", "TOWN_STATE_REJECTED"})
check("Tasks: task states match the lifecycle (0123: 13 states)", len(own.TaskState.values()) == 14)

# Machines (0133): an unset cap is distinguishable from clearing it
caps = rt(own.SetMachineCapsRequest(idempotency_key="k", machine_id="pi5", overcommit_cap=1.5))
check("Machines: SetMachineCaps changes only the caps it sets (optional fields)",
      caps.HasField("overcommit_cap") and not caps.HasField("host_reserve_mib"))

# Codex (0049, 0125, 0129)
prop = rt(own.CodexProposal(proposal_id="p1", tier=own.LAW_TIER_STATUTE, head_commit="abc123",
          state=own.CodexProposal.STATE_GATE_BLOCKED,
          eval=own.EvalReport(mode=own.EvalReport.MODE_CONSTITUTION, verdict=own.EvalReport.VERDICT_BLOCKED_REGRESSION,
                              failures=[own.EvalReport.Failure(scenario_id="adv-07", role="builder", passes=4, runs=5, adversarial=True)]),
          cedar=own.CedarReport(tests_passed=131, tests_total=131, tests_added=1)))
check("Codex: a proposal carries its eval gate and Cedar results",
      prop.eval.failures[0].adversarial and prop.cedar.tests_total == 131
      and prop.state == own.CodexProposal.STATE_GATE_BLOCKED)
check("Codex: signing names the exact commit the owner reviewed", "head_commit" in fields(own.SignProposalRequest))
check("Codex: the proposer's rationale is kept apart from the orchestrator's summary",
      {"summary", "author_rationale"} <= fields(own.CodexProposal))

# Restore (0135)
rv = rt(own.ReviewWindowRequest(idempotency_key="k", run_id="r1", findings=[
        own.CallFinding(call_id="01J0", finding=own.CallFinding.FINDING_RAN, observed_result="invoice sent", evidence="provider log"),
        own.CallFinding(call_id="01J1", finding=own.CallFinding.FINDING_DID_NOT_RUN)]))
check("Restore: owner findings round-trip per fenced call", [f.finding for f in rv.findings] == [1, 2])
check("Restore: a finding has no decision field, so a confirmed effect is never runnable (0135)",
      not ({"approve", "allow", "decision", "verdict"} & fields(own.CallFinding)))
ms = rt(own.MergeStatus(runs=[own.RunMergeStatus(run_id="r1", state=own.RunMergeStatus.STATE_AWAITING_REVIEW,
        merge=node.MergeResult(run_id="r1", accepted_through_seq=48, rejected_from_seq=49, reason="bad_signature"),
        fenced_calls=[own.FencedCall(call_id="01J0", tool="email.send")])]))
check("Restore: merge status reuses the machine's MergeResult and lists fenced calls",
      ms.runs[0].merge.rejected_from_seq == 49 and ms.runs[0].fenced_calls[0].tool == "email.send")

# Settings (0128, 0130)
ns = rt(own.NotificationSettings(relay_outage_alert=own.NotificationSettings.RelayOutageAlert(
        enabled=True, quiet_hours=own.QuietHours(**{"from": "23:00", "to": "07:00", "tz": "Europe/Sarajevo"}))))
check("Settings: relay outage alert can be on with quiet hours", ns.relay_outage_alert.enabled
      and ns.relay_outage_alert.quiet_hours.tz == "Europe/Sarajevo")
check("Settings: devices show their idle lock and whether they are locked",
      {"idle_lock_seconds", "locked"} <= fields(own.OwnerDevice))

# ---- forward compatibility (0083) ----
newer = ss.Heartbeat(monotonic_ms=5).SerializeToString() + bytes([0xF8, 0x0F, 0x01])   # unknown field 255
check("Forward compatibility: older peer ignores an unknown field", ss.Heartbeat.FromString(newer).monotonic_ms == 5)

print(f"\n{failures} failure(s)")
sys.exit(1 if failures else 0)
