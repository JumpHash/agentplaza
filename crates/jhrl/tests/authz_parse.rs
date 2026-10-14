//! Parsing of the authorization items of spec 9.13-9.14: operations, laws, fixtures and authz blocks.

use jhrl::ast::*;
use jhrl::parser::parse_module;

fn items(src: &str) -> Vec<Item> {
    parse_module(0, src)
        .unwrap_or_else(|d| panic!("parse failed: {}", d.msg))
        .items
}

const C004A: &str = r#"
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
"#;

#[test]
fn law_c004a_parses_its_header_scopes_and_conditions() {
    let it = items(C004A);
    let [Item::Law(l)] = it.as_slice() else {
        panic!("expected one law")
    };
    assert_eq!(l.id, "C-004a");
    assert_eq!(l.title, "Humans gate the irreversible");
    assert_eq!(l.tier.as_deref(), Some("constitution"));
    assert_eq!(l.enforcement.as_deref(), Some("preventive"));
    assert_eq!(l.severity.as_deref(), Some("critical"));
    assert_eq!(
        l.statement.as_deref(),
        Some("An irreversible tool call needs a human owner's approval.")
    );
    assert_eq!(l.effect, Effect::Forbid);
    assert_eq!(l.ops, vec!["tool.call".to_string()]);
    assert!(matches!(&l.principal, Some(Scope::Is(t)) if t.name() == Some("Agent")));
    assert!(matches!(&l.resource, Some(Scope::Is(t)) if t.name() == Some("Tool")));
    assert_eq!(l.when.len(), 1);
    assert_eq!(l.unless.len(), 1);
    assert!(l.expires.is_none() && l.town.is_none() && l.implements.is_empty());
}

#[test]
fn operation_signature_with_set_and_optional_context_fields() {
    let it = items(
        "operation tool.call(principal: Agent, resource: Tool, \
         context: {flags: Set<String>, owner_approval: Approval optional})",
    );
    let [Item::Operation(op)] = it.as_slice() else {
        panic!("expected one operation")
    };
    assert_eq!(op.name, "tool.call");
    assert_eq!(op.principal, vec!["Agent".to_string()]);
    assert_eq!(op.resource, "Tool");
    assert_eq!(op.context.len(), 2);
    let (f, fty, f_opt) = &op.context[0];
    assert_eq!(f, "flags");
    assert_eq!(fty.to_string(), "Set<String>");
    assert!(!f_opt);
    let (a, aty, a_opt) = &op.context[1];
    assert_eq!(a, "owner_approval");
    assert_eq!(aty.name(), Some("Approval"));
    assert!(*a_opt);
}

#[test]
fn operation_with_union_principal_and_record_context() {
    let it = items("operation task.assign(principal: Agent | Human, resource: Task, context: {limits: {cap: Int}})");
    let [Item::Operation(op)] = it.as_slice() else {
        panic!("expected one operation")
    };
    assert_eq!(op.principal, vec!["Agent".to_string(), "Human".to_string()]);
    assert!(matches!(&op.context[0].1, TypeExpr::Record(fs) if fs.len() == 1));
}

#[test]
fn fixture_and_authz_block_with_by_clause() {
    let src = r#"
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
    let it = items(src);
    assert_eq!(it.len(), 2);
    let Item::Fixture(fx) = &it[0] else {
        panic!("expected a fixture")
    };
    assert_eq!(fx.name, "f1");
    assert_eq!(fx.title, "Two agents and an approval");
    assert_eq!(fx.entities.len(), 3);
    assert_eq!(fx.entities[0].ty, "Agent");
    assert_eq!(fx.entities[0].id, "a1");
    assert_eq!(fx.entities[0].fields.len(), 1);
    assert_eq!(fx.entities[2].fields.len(), 0);

    let Item::Authz(az) = &it[1] else {
        panic!("expected an authz block")
    };
    assert_eq!(az.name, "gate");
    assert_eq!(az.fixture, "f1");
    assert_eq!(az.cases.len(), 2);
    let deny = &az.cases[0];
    assert!(!deny.allow);
    assert_eq!(deny.label, "no approval");
    assert_eq!(deny.subject.ty, "Agent");
    assert_eq!(deny.op, "tool.call");
    assert_eq!(deny.object.id, "t1");
    assert_eq!(deny.law.as_deref(), Some("C-004a"));
    let allow = &az.cases[1];
    assert!(allow.allow);
    assert_eq!(allow.law, None);
}

#[test]
fn law_without_effect_is_an_error() {
    let err = parse_module(0, r#"law "X" "t" { tier constitution permit any }"#);
    assert!(err.is_ok(), "permit any is a valid effect");
    let err = parse_module(0, r#"law "X" "t" { tier constitution }"#);
    assert!(err.is_err());
}
