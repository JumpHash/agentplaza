//! The JSON layer over apz-graph, built and tested on the native target.

use apz_graph::{Graph, Options};

/// Lays out `graph_json` (a graph: the read model of `GET /v1/projects/{id}/graph`, as it comes)
/// with `options_json` (the card size and gaps, any of them; empty or `null` for the desk sizes).
/// Returns the layout as JSON, or says what is wrong with the input.
pub fn layout(graph_json: &str, options_json: &str) -> Result<String, String> {
    let graph: Graph = serde_json::from_str(graph_json)
        .map_err(|err| format!("the graph does not read: {err}"))?;
    let options = match options_json.trim() {
        "" | "null" => Options::default(),
        text => {
            serde_json::from_str(text).map_err(|err| format!("the options do not read: {err}"))?
        }
    };
    options.check()?;
    serde_json::to_string(&apz_graph::layout(&graph, &options)).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn the_read_model_goes_in_as_it_comes() {
        let graph = json!({
            "project": "pr1",
            "stage": "implementation",
            "tasks": [
                {"id": "t1", "goal": "Design", "depth": 0, "state": "done", "ready": false},
                {"id": "t2", "goal": "Build", "depth": 1, "state": "open", "ready": true}
            ],
            "edges": [["t1", "t2"]],
            "frontier": ["t2"],
            "longest_chain": ["t2"],
            "waiting": []
        });
        let laid: Value =
            serde_json::from_str(&layout(&graph.to_string(), "").expect("a layout")).expect("JSON");
        assert_eq!(laid["columns"], json!([["t1"], ["t2"]]));
        assert_eq!(laid["cards"][1]["x"], 302.0);
        assert_eq!(laid["routes"][0]["from"], "t1");
        assert_eq!(laid["routes"][0]["points"][0], json!([256.0, 76.0]));
    }

    #[test]
    fn options_may_name_some_sizes_and_keep_the_rest() {
        let graph = json!({"tasks": [{"id": "a"}], "edges": []}).to_string();
        let laid: Value =
            serde_json::from_str(&layout(&graph, r#"{"card_width": 420}"#).expect("a layout"))
                .expect("JSON");
        assert_eq!(laid["cards"][0]["width"], 420.0);
        assert_eq!(laid["cards"][0]["height"], 104.0);
        assert!(layout(&graph, "null").is_ok());
    }

    #[test]
    fn input_that_cannot_be_laid_out_is_refused_with_why() {
        assert!(
            layout("not json", "")
                .unwrap_err()
                .starts_with("the graph does not read")
        );
        assert!(
            layout(r#"{"tasks": 3}"#, "")
                .unwrap_err()
                .starts_with("the graph does not read")
        );
        let graph = r#"{"tasks": []}"#;
        assert!(
            layout(graph, r#"{"card_width": "wide"}"#)
                .unwrap_err()
                .starts_with("the options do not read")
        );
        assert_eq!(
            layout(graph, r#"{"card_width": 0}"#).unwrap_err(),
            "card_width must be more than 0"
        );
        assert_eq!(
            layout(graph, r#"{"gap_y": 40, "pad": 10}"#).unwrap_err(),
            "pad must be at least half of gap_y (20), so a channel stays inside the layout"
        );
    }
}
