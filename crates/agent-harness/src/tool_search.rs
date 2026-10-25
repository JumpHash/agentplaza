//! Deferred tools (PLAN 13.3): the profile offers some tools by name and one line only, so the model's tool list stays
//! short. The model fetches a tool's schema through `tool_search` before it calls the tool; `tool_search` is served
//! here, and a call to a deferred tool that was not fetched is refused here. The fetched schemas stay in the request's
//! tools for the rest of the work item.

use std::collections::{BTreeMap, BTreeSet};

use apz_proto::chat::ToolSpec;
use serde_json::json;

/// The tool the harness serves itself. The hub offers it as `apid::tools::TOOL_SEARCH`.
pub const TOOL: &str = "tool_search";

/// The most schemas one search returns; a broader query says so and the model narrows it.
const MAX_RESULTS: usize = 10;

/// One work item's deferred tools, with their full schemas, and the names the model has fetched.
#[derive(Debug, Default)]
pub struct Deferred {
    schemas: BTreeMap<String, ToolSpec>,
    fetched: BTreeSet<String>,
}

impl Deferred {
    /// The deferred tools of `tools` (the profile's, with their full schemas); none is fetched yet.
    pub fn new(tools: &[ToolSpec]) -> Self {
        Deferred {
            schemas: tools
                .iter()
                .filter(|tool| tool.deferred)
                .map(|tool| (tool.name.clone(), tool.clone()))
                .collect(),
            fetched: BTreeSet::new(),
        }
    }

    /// The tools a model call offers: `tools` in order, a deferred tool with an empty schema until it is fetched and
    /// with its full schema after.
    pub fn request_tools(&self, tools: &[ToolSpec]) -> Vec<ToolSpec> {
        tools
            .iter()
            .map(|tool| {
                if self.fetched.contains(&tool.name) {
                    let full = self.schemas.get(&tool.name).unwrap_or(tool);
                    ToolSpec {
                        deferred: false,
                        ..full.clone()
                    }
                } else if tool.deferred {
                    ToolSpec {
                        input_schema: json!({"type": "object"}),
                        ..tool.clone()
                    }
                } else {
                    tool.clone()
                }
            })
            .collect()
    }

    /// Why a call to `name` is refused, when it is a deferred tool that was not fetched.
    pub fn refusal(&self, name: &str) -> Option<String> {
        (self.schemas.contains_key(name) && !self.fetched.contains(name))
            .then(|| format!("{name} has no schema yet: fetch its schema with tool_search first"))
    }

    /// `tool_search`'s answer to `query`: the deferred tools whose name or description has one of its words, with their
    /// schemas. Those tools are fetched from now on.
    pub fn search(&mut self, query: &str) -> String {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let matches: Vec<ToolSpec> = self
            .schemas
            .values()
            .filter(|tool| {
                let text = format!("{} {}", tool.name, tool.description).to_lowercase();
                words.iter().any(|word| text.contains(word.as_str()))
            })
            .cloned()
            .collect();
        if matches.is_empty() {
            return format!("no deferred tool matches \"{query}\"");
        }
        let mut answer: Vec<String> = Vec::new();
        for tool in matches.iter().take(MAX_RESULTS) {
            self.fetched.insert(tool.name.clone());
            answer.push(format!(
                "{}: {}\ninput schema: {}",
                tool.name, tool.description, tool.input_schema
            ));
        }
        if matches.len() > MAX_RESULTS {
            answer.push(format!(
                "{} more tools match \"{query}\": narrow the query to see them",
                matches.len() - MAX_RESULTS
            ));
        }
        answer.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str, deferred: bool) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            deferred,
        }
    }

    fn offered() -> Vec<ToolSpec> {
        vec![
            tool("run", "run a command", false),
            tool("ledger_rows", "read the ledger rows", true),
        ]
    }

    #[test]
    fn a_deferred_tool_is_offered_by_name_until_its_schema_is_fetched() {
        let deferred = Deferred::new(&offered());
        let before = deferred.request_tools(&offered());
        assert_eq!(before[0], offered()[0]);
        assert_eq!(before[1].input_schema, json!({"type": "object"}));
        assert!(before[1].deferred);
        assert!(deferred.refusal("ledger_rows").is_some());
        assert!(deferred.refusal("run").is_none());
    }

    #[test]
    fn a_search_returns_the_schema_and_the_next_request_carries_it() {
        let mut deferred = Deferred::new(&offered());
        let answer = deferred.search("ledger");
        assert!(
            answer.starts_with("ledger_rows: read the ledger rows"),
            "{answer}"
        );
        assert!(answer.contains(r#""path""#), "{answer}");
        assert!(deferred.refusal("ledger_rows").is_none());
        let after = deferred.request_tools(&offered());
        assert_eq!(after[1], tool("ledger_rows", "read the ledger rows", false));
    }

    #[test]
    fn a_search_that_matches_nothing_fetches_nothing() {
        let mut deferred = Deferred::new(&offered());
        assert_eq!(
            deferred.search("calendar"),
            r#"no deferred tool matches "calendar""#
        );
        assert!(deferred.refusal("ledger_rows").is_some());
    }
}
