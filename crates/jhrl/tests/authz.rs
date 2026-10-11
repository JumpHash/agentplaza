//! Authorization decisions of spec 9.13-9.14: the C-004a example, forbid over permit, default
//! deny, and fail-closed handling of a missing attribute.

use jhrl::authz::{authorize, entity_value, run_authz, store_for, Decision, Entity, Request};
use jhrl::program::Program;
use jhrl::value::Value;

const SRC: &str = r#"
operation tool.call(principal: Agent, resource: Tool, context: {flags: Set<String>, owner_approval: Approval optional})

law "C-004a" "Humans gate the irreversible" {
  tier constitution
  enforcement preventive
  severity critical
  statement "An irreversible tool call needs a human owner's approval."
  forbid tool.call
  principal is Agent
  resource is Tool
  when resource.risk_class == "irreversible"
  unless has(context.owner_approval)
}

law "P-001" "Agents may call tools" {
  tier agent
  enforcement preventive
  permit tool.call
  principal is Agent
  resource is Tool
}

fixture "f1" "Two agents and an approval" {
  Agent("a1") { role: "builder" }
  Tool("t1") { risk_class: "irreversible" }
  Approval("ap1") { }
}

authz "gate" using "f1" {
  deny "no approval" Agent("a1") tool.call Tool("t1") {flags: {"x"}, owner_approval: none} by "C-004a"
  allow "approved" Agent("a1") tool.call Tool("t1") {flags: {}, owner_approval: Approval("ap1")}
}
"#;

const FAIL_CLOSED: &str = r#"
operation tool.call(principal: Agent, resource: Tool, context: {flags: Set<String>})

law "F-001" "Secret tools are closed" {
  tier agent
  enforcement preventive
  forbid tool.call
  principal is Agent
  resource is Tool
  when resource.classification == "secret"
}

law "P-002" "Public tools are open" {
  tier agent
  enforcement preventive
  permit tool.call
  principal is Agent
  resource is Tool
  when resource.classification == "public"
}

fixture "f1" "One agent, one tool" {
  Agent("a1") { role: "builder" }
  Tool("t1") { risk_class: "irreversible" }
}
"#;

fn load(src: &str) -> Program {
    let (p, d) = Program::load(vec![("authz.jhrl".to_string(), src.to_string())]);
    assert!(d.iter().all(|x| x.warning), "load diagnostics: {d:?}");
    p
}

fn request(op: &str, pairs: Vec<(&str, Value)>) -> Request {
    Request {
        principal: Entity { ty: "Agent".into(), id: "a1".into() },
        operation: op.into(),
        resource: Entity { ty: "Tool".into(), id: "t1".into() },
        context: Value::record(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()),
    }
}

fn approval() -> Value {
    entity_value(&Entity { ty: "Approval".into(), id: "ap1".into() })
}

#[test]
fn c004a_denies_irreversible_without_approval_and_not_with_one() {
    let p = load(SRC);
    let s = store_for(&p, "f1").expect("fixture f1");
    let no = authorize(&p, &s, &request("tool.call", vec![]));
    assert_eq!(no.decision, Decision::Deny { by: vec!["C-004a".into()], missing: vec![] });
    let yes = authorize(&p, &s, &request("tool.call", vec![("owner_approval", approval())]));
    assert_eq!(yes.decision, Decision::Allow { by: vec!["P-001".into()] });
    let rows = run_authz(&p);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(_, pass, _)| *pass), "{rows:?}");
}

#[test]
fn forbid_beats_permit() {
    let p = load(SRC);
    let s = store_for(&p, "f1").expect("fixture f1");
    // Both C-004a (forbid) and P-001 (permit) apply here; the forbid wins and P-001 is not cited.
    let o = authorize(&p, &s, &request("tool.call", vec![]));
    assert_eq!(o.decision, Decision::Deny { by: vec!["C-004a".into()], missing: vec![] });
    assert!(o.violations.is_empty(), "C-004a is preventive, not a violation record");
}

#[test]
fn default_deny_when_no_law_covers_the_operation() {
    let p = load(SRC);
    let s = store_for(&p, "f1").expect("fixture f1");
    let o = authorize(&p, &s, &request("tool.delete", vec![]));
    assert_eq!(o.decision, Decision::Deny { by: vec![], missing: vec![] });
}

#[test]
fn forbid_on_missing_attribute_denies_and_unknown_permit_does_not_apply() {
    let p = load(FAIL_CLOSED);
    let s = store_for(&p, "f1").expect("fixture f1");
    // Tool t1 has no `classification`: F-001 is Unknown and applies (fail closed); P-002 is
    // Unknown and does not apply, so it is not cited.
    let o = authorize(&p, &s, &request("tool.call", vec![]));
    assert_eq!(
        o.decision,
        Decision::Deny { by: vec!["F-001".into()], missing: vec!["resource.classification".into()] }
    );
}
