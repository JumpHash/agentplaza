//! Agent Trace on demand: a change's line ranges and who wrote them, in a form other tools read. Nothing of it goes
//! to git (W13); it is produced only when asked for.

use serde_json::{Value, json};

use crate::derived::Derived;
use crate::record::Actor;
use crate::store::Store;

pub fn trace(store: &Store, derived: &Derived, change: &str) -> Result<Value, String> {
    let rev = {
        let st = store.state();
        let c = st.change(change)?;
        c.landed
            .clone()
            .or_else(|| c.rounds.last().cloned())
            .or_else(|| c.drafts.last().cloned())
            .ok_or("the change has no revision")?
    };
    let tree = derived.change_tree(store, &rev)?;
    let mut files = Vec::new();
    for f in &tree.files {
        let mut ranges = Vec::new();
        for s in &f.symbols {
            for p in &s.provenance {
                let contributor = match &p.by {
                    Actor::Agent { cid } => {
                        json!({ "type": "ai", "agent": format!("agent:{cid}"), "model_id": p.model })
                    }
                    Actor::Owner => json!({ "type": "human", "name": p.author }),
                    other => json!({ "type": "system", "name": other.label() }),
                };
                ranges.push(json!({ "start_line": p.start_line, "end_line": p.end_line, "symbol": s.qualified, "contributor": contributor, "turn": p.turn, "tool_calls": p.tool_calls, "revision": p.source }));
            }
        }
        files.push(json!({ "path": f.path, "ranges": ranges }));
    }
    Ok(
        json!({ "version": "0.1", "vcs": { "type": "weft", "revision": rev }, "change": change, "files": files }),
    )
}
