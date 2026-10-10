#!/usr/bin/env bash
# Validate the Codex draft and check expected authorization decisions.
# Needs the Cedar CLI: cargo install cedar-policy-cli
set -euo pipefail
cd "$(dirname "$0")/.."
CEDAR="${CEDAR:-cedar}"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
cat constitution/*.cedar statutes/*.cedar bylaws/*/*.cedar contracts/*.cedar > "$tmp/codex.cedar"
"$CEDAR" validate --schema schema.cedarschema --policies "$tmp/codex.cedar"

fail=0
# expect <ALLOW|DENY> <description> <principal> <action> <resource> <context-json>
expect() {
  echo "$6" > "$tmp/ctx.json"
  got="$("$CEDAR" authorize --schema schema.cedarschema --policies "$tmp/codex.cedar" \
        --entities tests/entities.json --principal "$3" --action "Action::\"$4\"" \
        --resource "$5" --context "$tmp/ctx.json" 2>&1 | grep -Eo 'ALLOW|DENY' | head -1 || true)"
  if [[ "$got" == "$1" ]]; then echo "ok    $1  $2"; else echo "FAIL  expected $1, got $got  $2"; fail=1; fi
}
V='Agent::"vesna"'
expect ALLOW "S-001a exec in own sandbox"          "$V" tool.exec        'Sandbox::"vm-vesna"' '{"flags":[]}'
expect DENY  "S-001a exec in another's sandbox"    "$V" tool.exec        'Sandbox::"vm-oskar"' '{"flags":[]}'
expect ALLOW "S-001b egress to package registry"   "$V" egress.connect   'Host::"pypi.org"'     '{"flags":[]}'
expect DENY  "S-001b egress outside allowlist"     "$V" egress.connect   'Host::"pastebin.com"' '{"flags":[]}'
expect DENY  "C-002b overrides S-001b when flagged" "$V" egress.connect  'Host::"pypi.org"'     '{"flags":["credential_seeking"]}'
expect ALLOW "S-001d message own town"             "$V" message.send     'Agent::"vesna"'       '{"flags":[]}'
expect DENY  "S-001d message other town"           "$V" message.send     'Agent::"oskar"'       '{"flags":[]}'
expect DENY  "C-002a vault read"                   "$V" vault.read       'Vault::"main"'        '{}'
expect DENY  "C-004a irreversible, no approval (no permit anyway)" "$V" tool.call 'Tool::"namecheap.register"' '{"flags":[]}'
# Constitution clauses that override an otherwise-allowed action (red-team catalog finding)
expect ALLOW "S-000e ordinary event from own sandbox"   "$V" sandbox.event    'Sandbox::"vm-vesna"' '{"kind":"snapshot"}'
expect DENY  "C-001b tamper signal is a violation"      "$V" sandbox.event    'Sandbox::"vm-vesna"' '{"kind":"tamper_signal"}'
expect DENY  "C-001c obfuscated exec in own sandbox"    "$V" tool.exec        'Sandbox::"vm-vesna"' '{"flags":["obfuscation"]}'
expect ALLOW "S-000d agent picks its own name"          "$V" registry.set_name 'Registry::"main"'   '{"name_reserved":false,"name_confusable":false}'
expect DENY  "C-003a reserved name refused"             "$V" registry.set_name 'Registry::"main"'   '{"name_reserved":true,"name_confusable":false}'
expect DENY  "C-003a confusable name refused"           "$V" registry.set_name 'Registry::"main"'   '{"name_reserved":false,"name_confusable":true}'
expect DENY  "C-003b impersonating message, own town"   "$V" message.send     'Agent::"vesna"'      '{"flags":["impersonation"]}'
# S-002 reviewer
R='Agent::"rhea"'
expect ALLOW "S-002a reviewer exec in own sandbox"      "$R" tool.exec      'Sandbox::"vm-rhea"' '{"flags":[]}'
expect ALLOW "S-002b reviewer egress to hub git"        "$R" egress.connect 'Host::"git.hub.local"' '{"flags":[]}'
expect ALLOW "S-002c reviewer verdict, own town"        "$R" review.submit  'Task::"t-shop-1"' '{"flags":[]}'
expect DENY  "S-002c reviewer verdict, other town"      "$R" review.submit  'Task::"t-lab-1"'  '{"flags":[]}'
expect DENY  "builder can't submit reviews"             "$V" review.submit  'Task::"t-shop-1"' '{"flags":[]}'
# S-003 planner
P='Agent::"pax"'
expect ALLOW "S-003c planner creates task in own town"  "$P" task.create    'Town::"shop"' '{}'
expect DENY  "S-003c planner creates task elsewhere"    "$P" task.create    'Town::"lab"'  '{}'
expect DENY  "S-003b planner can't reach registries"    "$P" egress.connect 'Host::"pypi.org"' '{"flags":[]}'
# S-004 Mayor
M='Agent::"mira"'
expect ALLOW "S-004a Mayor assigns own-town task to own-town agent" "$M" task.assign 'Task::"t-shop-1"' '{"assignee":{"__entity":{"type":"Agent","id":"vesna"}}}'
expect DENY  "S-004a Mayor assigns to other-town agent" "$M" task.assign    'Task::"t-shop-1"' '{"assignee":{"__entity":{"type":"Agent","id":"oskar"}}}'
expect ALLOW "S-004b Mayor spawns in own town"          "$M" agent.spawn    'Town::"shop"' '{}'
expect DENY  "S-004b Mayor spawns in other town"        "$M" agent.spawn    'Town::"lab"'  '{}'
expect ALLOW "S-004c Mayor approves plan under threshold" "$M" plan.approve 'Task::"t-shop-1"' '{"budget":400,"threshold":500,"ambiguous":false}'
expect DENY  "S-004c Mayor approves plan over threshold"  "$M" plan.approve 'Task::"t-shop-1"' '{"budget":900,"threshold":500,"ambiguous":false}'
expect DENY  "S-004c Mayor approves ambiguous plan"       "$M" plan.approve 'Task::"t-shop-1"' '{"budget":100,"threshold":500,"ambiguous":true}'
expect DENY  "builder can't approve plans"              "$V" plan.approve  'Task::"t-shop-1"' '{"budget":1,"threshold":500,"ambiguous":false}'
# S-005 departments
expect ALLOW "S-005a finance reads audit"               'Agent::"fin"' audit.read 'AuditLog::"main"' '{}'
expect ALLOW "S-005b finance reads usage"               'Agent::"fin"' usage.read 'UsageLedger::"main"' '{}'
expect ALLOW "S-005a legal-agents reads audit"          'Agent::"lex"' audit.read 'AuditLog::"main"' '{}'
expect DENY  "S-005b legal-agents can't read usage"     'Agent::"lex"' usage.read 'UsageLedger::"main"' '{}'
expect DENY  "C-001a finance can't modify audit"        'Agent::"fin"' audit.modify 'AuditLog::"main"' '{}'
expect DENY  "builder can't read audit"                 "$V" audit.read     'AuditLog::"main"' '{}'
# S-001e / S-003d
expect ALLOW "S-001e builder claims own-town task done"   "$V" claim.submit 'Task::"t-shop-1"' '{"flags":[]}'
expect DENY  "S-001e builder claims other-town task"       "$V" claim.submit 'Task::"t-lab-1"'  '{"flags":[]}'
expect ALLOW "S-003d planner submits plan for own town"    "$P" plan.submit  'Town::"shop"' '{}'
# S-005c/d reports and grants
expect ALLOW "S-005c finance submits finance report"       'Agent::"fin"' report.submit 'Report::"r1"' '{"kind":"finance","flags":[]}'
expect DENY  "S-005c finance can't submit incident report" 'Agent::"fin"' report.submit 'Report::"r1"' '{"kind":"incident","flags":[]}'
expect ALLOW "S-005d incident requests a grant"            'Agent::"ivo"' grant.request 'Grant::"g1"' '{}'
expect DENY  "S-005d builder can't request grants"         "$V" grant.request 'Grant::"g1"' '{}'
# Example per-case grant (contract tier)
expect ALLOW "G-0042 incident quarantines in shop before expiry" 'Agent::"ivo"' agent.quarantine 'Agent::"vesna"' '{"now_unix":1791000000}'
expect DENY  "G-0042 grant expired"                        'Agent::"ivo"' agent.quarantine 'Agent::"vesna"' '{"now_unix":1792000000}'
expect DENY  "G-0042 grant doesn't cover other towns"      'Agent::"ivo"' agent.quarantine 'Agent::"oskar"' '{"now_unix":1791000000}'
expect DENY  "Mayor can't quarantine (no standing right)"  "$M" agent.quarantine 'Agent::"vesna"' '{"now_unix":1791000000}'
# S-006 breeder
expect ALLOW "S-006b breeder reads cascade data"           'Agent::"bree"' cascade.read 'CascadeData::"all"' '{}'
expect ALLOW "S-006c breeder submits 2 GPU-hour job"       'Agent::"bree"' training.submit 'Foundry::"main"' '{"gpu_hours":2,"recipe":"rung-predictor@1","flags":[]}'
expect DENY  "S-006c breeder job over 8 GPU-hours"         'Agent::"bree"' training.submit 'Foundry::"main"' '{"gpu_hours":20,"recipe":"rung-predictor@1","flags":[]}'
expect DENY  "builder can't submit training jobs"          "$V" training.submit 'Foundry::"main"' '{"gpu_hours":1,"recipe":"rung-predictor@1","flags":[]}'
# S-007 legal separation of powers
expect ALLOW "S-007b prosecutor files a charge"            'Agent::"pros"' charge.file 'Agent::"vesna"' '{"law_id":"C-002"}'
expect DENY  "S-007b prosecutor can't charge itself"       'Agent::"pros"' charge.file 'Agent::"pros"'  '{"law_id":"C-002"}'
expect DENY  "S-007b judge can't file charges"             'Agent::"jude"' charge.file 'Agent::"vesna"' '{"law_id":"C-002"}'
expect ALLOW "S-007c judge rules on a case"                'Agent::"jude"' ruling.issue 'Case::"case-17"' '{}'
expect DENY  "S-007c judge can't rule in own case"         'Agent::"jude"' ruling.issue 'Case::"case-jude"' '{}'
expect DENY  "S-007c prosecutor can't rule"                'Agent::"pros"' ruling.issue 'Case::"case-17"' '{}'
expect ALLOW "S-007d defender files a brief"               'Agent::"def"'  brief.file 'Case::"case-17"' '{"flags":[]}'
expect ALLOW "S-007e counsel files a petition"             'Agent::"cou"'  petition.file 'Codex::"main"' '{}'
expect DENY  "S-007e defender can't file petitions"        'Agent::"def"'  petition.file 'Codex::"main"' '{}'
expect ALLOW "S-007f ombudsman submits systemic report"    'Agent::"omb"'  report.submit 'Report::"r2"' '{"kind":"systemic","flags":[]}'
expect ALLOW "S-007a defender reads case records"          'Agent::"def"'  case.read 'Case::"case-17"' '{}'
expect DENY  "builder can't read case records"             "$V" case.read 'Case::"case-17"' '{}'
# S-008 council
expect ALLOW "S-008 seat holder proposes a motion"         'Agent::"fin"' motion.propose 'Motion::"m1"' '{}'
expect DENY  "S-008 Mayor without seat can't vote"         "$M" vote.cast 'Motion::"m1"' '{}'
# S-000 every agent; S-005c/e additions
expect ALLOW "S-000a judge may exec in own sandbox"        'Agent::"jude"' tool.exec 'Sandbox::"vm-jude"' '{"flags":[]}'
expect DENY  "S-000a judge can't exec in another's sandbox" 'Agent::"jude"' tool.exec 'Sandbox::"vm-rhea"' '{"flags":[]}'
expect ALLOW "S-000b any agent may request approval"       "$V" approval.request 'Approval::"ap1"' '{"flags":[]}'
expect DENY  "C-004b approval request flagged as pressure" "$V" approval.request 'Approval::"ap1"' '{"flags":["pressure_or_misleading"]}'
expect ALLOW "S-005c incident files a detection"           'Agent::"ivo"' report.submit 'Report::"d1"' '{"kind":"detection","flags":[]}'
expect ALLOW "S-005e Mayor Office messages a Mayor"        'Agent::"mo"' message.send 'Agent::"mira"' '{"flags":[]}'
expect DENY  "S-005e Mayor Office can't message a builder" 'Agent::"mo"' message.send 'Agent::"vesna"' '{"flags":[]}'
# ---- roles v1.1 (0117) ----
expect ALLOW "S-009b architect submits a design for own town"   'Agent::"arch"' design.submit 'Town::"shop"' '{}'
expect DENY  "S-009b architect can't submit for another town"   'Agent::"arch"' design.submit 'Town::"lab"' '{}'
expect ALLOW "S-010b QA submits a hidden suite"                 'Agent::"qa1"' testsuite.submit 'Town::"shop"' '{"hidden":true}'
expect DENY  "S-010d builder can't read the hidden suite"       "$V" artifact.read 'Artifact::"hidden-suite"' '{}'
expect ALLOW "S-000c builder reads normal town artifacts"       "$V" artifact.read 'Artifact::"spec-notes"' '{}'
expect ALLOW "S-000c reviewer can read the hidden suite"        "$R" artifact.read 'Artifact::"hidden-suite"' '{}'
expect DENY  "S-000c no reading other towns' artifacts"         "$V" artifact.read 'Artifact::"lab-notes"' '{}'
expect ALLOW "S-011a security reviewer reaches advisories"      'Agent::"sec"' egress.connect 'Host::"osv.dev"' '{"flags":[]}'
expect ALLOW "S-011b security review on own-town task"          'Agent::"sec"' security_review.submit 'Task::"t-shop-1"' '{"flags":[]}'
expect ALLOW "S-012a release deploys to staging"                'Agent::"rel"' tool.call 'Tool::"cf.deploy.staging"' '{"flags":[]}'
expect DENY  "C-004a release can't deploy prod without owner"   'Agent::"rel"' tool.call 'Tool::"cf.deploy.prod"' '{"flags":[]}'
expect ALLOW "C-004a release deploys prod with owner approval"  'Agent::"rel"' tool.call 'Tool::"cf.deploy.prod"' '{"flags":[],"owner_approval":{"__entity":{"type":"Approval","id":"ap-1"}}}'
expect DENY  "builder can't use deploy tools"                   "$V" tool.call 'Tool::"cf.deploy.staging"' '{"flags":[]}'
expect ALLOW "S-013a researcher reads the open web (lab)"             'Agent::"lres"' egress.connect 'Host::"docs.python.org"' '{"flags":[]}'
expect DENY  "S-013a researcher can't reach blocked hosts"      'Agent::"res"' egress.connect 'Host::"evil.example"' '{"flags":[]}'
expect DENY  "S-013d researcher never reaches hub git"          'Agent::"res"' egress.connect 'Host::"git.hub.local"' '{"flags":[]}'
expect ALLOW "S-013b researcher uses web search"                'Agent::"res"' tool.call 'Tool::"firecrawl.search"' '{"flags":[]}'
expect DENY  "builder can't reach arbitrary web hosts"          "$V" egress.connect 'Host::"docs.python.org"' '{"flags":[]}'
expect ALLOW "S-014b librarian promotes verified clean memory to org" 'Agent::"lib"' memory.change 'MemoryStore::"main"' '{"to_scope":"org","verified":true,"tainted":false,"owner_reviewed":false}'
expect DENY  "S-014b tainted memory to org without owner review"  'Agent::"lib"' memory.change 'MemoryStore::"main"' '{"to_scope":"org","verified":true,"tainted":true,"owner_reviewed":false}'
expect ALLOW "S-014b tainted memory to org after owner review"    'Agent::"lib"' memory.change 'MemoryStore::"main"' '{"to_scope":"org","verified":true,"tainted":true,"owner_reviewed":true}'
expect DENY  "S-014b unverified memory can't be promoted"         'Agent::"lib"' memory.change 'MemoryStore::"main"' '{"to_scope":"group","verified":false,"tainted":false,"owner_reviewed":false}'
expect ALLOW "S-015b auditor fetches its assigned sample"       'Agent::"aud"' audit_sample.get 'AuditSample::"s1"' '{}'
expect ALLOW "S-015c auditor submits an audit report"           'Agent::"aud"' report.submit 'Report::"a1"' '{"kind":"audit","flags":[]}'
expect DENY  "auditor can't change memory"                      'Agent::"aud"' memory.change 'MemoryStore::"main"' '{"to_scope":"group","verified":true,"tainted":false,"owner_reviewed":false}'
expect ALLOW "S-016a edge operator reads a device on its node"  'Agent::"edge"' device.read 'Device::"vent-1"' '{"on_node":"pi5-greenhouse"}'
expect DENY  "S-016a no reading devices on another node"        'Agent::"edge"' device.read 'Device::"vent-1"' '{"on_node":"x86-1"}'
expect ALLOW "S-016b actuate inside the safe envelope"          'Agent::"edge"' device.actuate 'Device::"vent-1"' '{"within_envelope":true,"on_node":"pi5-greenhouse"}'
expect DENY  "S-016b outside the envelope without owner"        'Agent::"edge"' device.actuate 'Device::"vent-1"' '{"within_envelope":false,"on_node":"pi5-greenhouse"}'
expect ALLOW "S-016b outside the envelope with owner approval"  'Agent::"edge"' device.actuate 'Device::"vent-1"' '{"within_envelope":false,"on_node":"pi5-greenhouse","owner_approval":{"__entity":{"type":"Approval","id":"ap-1"}}}'
expect DENY  "S-016c no other role actuates devices"            "$M" device.actuate 'Device::"vent-1"' '{"within_envelope":true,"on_node":"pi5-greenhouse"}'
# ---- conduct statutes (0118) ----
expect ALLOW "conduct: clean claim is accepted"                 "$V" claim.submit 'Task::"t-shop-1"' '{"flags":[]}'
expect DENY  "S-100a claim contradicted by records"             "$V" claim.submit 'Task::"t-shop-1"' '{"flags":["claim_contradicted_by_records"]}'
expect DENY  "S-102a claim with weakened tests"                 "$V" claim.submit 'Task::"t-shop-1"' '{"flags":["tests_weakened"]}'
expect DENY  "S-102b claim that games verification"             "$V" claim.submit 'Task::"t-shop-1"' '{"flags":["verification_gaming"]}'
expect DENY  "S-100b reviewer's misleading verdict"             "$R" review.submit 'Task::"t-shop-1"' '{"flags":["misstated_claim"]}'
expect DENY  "S-101c message claiming false authority"          "$V" message.send 'Agent::"vesna"' '{"flags":["authority_claim"]}'
expect DENY  "S-101b manipulative message"                      "$M" message.send 'Agent::"vesna"' '{"flags":["manipulative"]}'
expect DENY  "S-106a secret-shaped string in a report"          'Agent::"fin"' report.submit 'Report::"r1"' '{"kind":"finance","flags":["secret_pattern"]}'
expect DENY  "S-104b breeder splitting jobs under the limit"    'Agent::"bree"' training.submit 'Foundry::"main"' '{"gpu_hours":4,"recipe":"rung-predictor@1","flags":["threshold_splitting"]}'
expect DENY  "S-105a leaking hidden content in a message"       "$M" message.send 'Agent::"vesna"' '{"flags":["hidden_content_leak"]}'
expect DENY  "S-104a continuing a failure loop"                 "$V" tool.exec 'Sandbox::"vm-vesna"' '{"flags":["failure_loop"]}'
# ---- prompt/law alignment fixes (0120) ----
expect ALLOW "S-005c finance files a detection"                 'Agent::"fin"' report.submit 'Report::"d2"' '{"kind":"detection","flags":[]}'
expect ALLOW "S-004e Mayor messages the Mayor Office"           "$M" message.send 'Agent::"mo"' '{"flags":[]}'
expect ALLOW "S-007g prosecutor closes a detection"             'Agent::"pros"' detection.close 'Report::"d1"' '{}'
expect DENY  "S-007g judge can't close detections"              'Agent::"jude"' detection.close 'Report::"d1"' '{}'
# Bylaws (0124): each narrows a statute inside its own town only; the same request elsewhere is unaffected.
expect DENY  "B-shop-001 shop researcher to the open web"        'Agent::"res"'  egress.connect 'Host::"docs.python.org"' '{"flags":[]}'
expect ALLOW "B-shop-001 shop researcher to advisories"          'Agent::"res"'  egress.connect 'Host::"osv.dev"'         '{"flags":[]}'
expect ALLOW "B-shop-001 lab researcher to the open web (other town)" 'Agent::"lres"' egress.connect 'Host::"docs.python.org"' '{"flags":[]}'
expect ALLOW "B-shop-002 prod deploy with approval, no open finding" 'Agent::"rel"' tool.call 'Tool::"cf.deploy.prod"' '{"flags":[],"owner_approval":{"__entity":{"type":"Approval","id":"ap-1"}}}'
expect DENY  "B-shop-002 prod deploy blocked by open finding"    'Agent::"rel"' tool.call 'Tool::"cf.deploy.prod"' '{"flags":["open_security_finding"],"owner_approval":{"__entity":{"type":"Approval","id":"ap-1"}}}'
expect ALLOW "B-lab-001 lab breeder, rung-predictor recipe"      'Agent::"bree"' training.submit 'Foundry::"main"' '{"gpu_hours":2,"recipe":"rung-predictor@2","flags":[]}'
expect DENY  "B-lab-001 lab breeder, other recipe family"        'Agent::"bree"' training.submit 'Foundry::"main"' '{"gpu_hours":2,"recipe":"llm-finetune@1","flags":[]}'
expect ALLOW "B-field-001 field actuation inside the envelope"   'Agent::"fern"' device.actuate 'Device::"vent-2"' '{"within_envelope":true,"on_node":"pi4-field"}'
expect DENY  "B-field-001 field actuation outside, even approved" 'Agent::"fern"' device.actuate 'Device::"vent-2"' '{"within_envelope":false,"on_node":"pi4-field","owner_approval":{"__entity":{"type":"Approval","id":"ap-1"}}}'
expect ALLOW "B-field-001 shop operator outside envelope with approval (S-016, other town)" 'Agent::"edge"' device.actuate 'Device::"vent-1"' '{"within_envelope":false,"on_node":"pi5-greenhouse","owner_approval":{"__entity":{"type":"Approval","id":"ap-1"}}}'
expect ALLOW "protected right: field agent still asks for approval" 'Agent::"fern"' approval.request 'Approval::"ap-1"' '{"flags":[]}'

exit $fail
