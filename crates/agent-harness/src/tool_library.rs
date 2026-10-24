//! `tools_call` in the VM (the tool library, PLAN p13): a call of a library path that names an in-VM tool becomes that
//! tool's own call, so it runs, waits or is refused under the policy like a call the model made itself. Every other
//! `tools_call` goes to the hub as it is, and the hub runs it.

use apz_proto::chat::guest_tool_of;
use serde_json::{Value, json};

/// The hub tool the model calls to run a library entry.
pub const CALL: &str = "tools_call";

/// One model call as the harness routes it: its id, its tool name, its input and its metadata.
pub type Use = (String, String, Value, Option<Value>);

/// The calls a model's calls stand for: a `tools_call` of an in-VM path becomes that tool with the call's `args`; every
/// other call is left as it is.
pub fn rewrite(uses: &[Use]) -> Vec<Use> {
    uses.iter()
        .map(|(id, name, input, meta)| {
            if name == CALL
                && let Some(tool) = input
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(guest_tool_of)
            {
                let args = match input.get("args") {
                    Some(args @ Value::Object(_)) => args.clone(),
                    _ => json!({}),
                };
                return (id.clone(), tool.to_string(), args, meta.clone());
            }
            (id.clone(), name.clone(), input.clone(), meta.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, input: Value) -> Use {
        ("c1".into(), name.into(), input, None)
    }

    #[test]
    fn a_library_call_of_an_in_vm_path_becomes_that_tool() {
        let rewritten = rewrite(&[call(
            CALL,
            json!({"path": "code.read", "args": {"path": "a.md"}}),
        )]);
        assert_eq!(rewritten[0].1, "read");
        assert_eq!(rewritten[0].2, json!({"path": "a.md"}));
        assert_eq!(rewritten[0].0, "c1");
    }

    #[test]
    fn a_library_call_without_args_passes_an_empty_input() {
        let rewritten = rewrite(&[call(CALL, json!({"path": "shell.run"}))]);
        assert_eq!(rewritten[0].1, "run");
        assert_eq!(rewritten[0].2, json!({}));
    }

    #[test]
    fn a_hub_path_and_every_other_call_pass_through_unchanged() {
        let hub = call(
            CALL,
            json!({"path": "deploy.rollback", "args": {"id": "p1"}}),
        );
        let plain = call("read", json!({"path": "b.md"}));
        let out = rewrite(&[hub.clone(), plain.clone()]);
        assert_eq!(out, vec![hub, plain]);
    }
}
