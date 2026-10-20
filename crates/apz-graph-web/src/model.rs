//! The JSON layer over apz-graph, built and tested on the native target.

use std::collections::{BTreeMap, HashMap};

use apz_graph::force::{self, ForceOptions};
use apz_graph::{Graph, Options};
use serde_json::{Value, json};

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

/// Lays out a graph's nodes and links with the force-directed layout. The input is JSON: `nodes` (ids),
/// `edges` (`[from, to]` pairs of ids), `width` and `height`, and optionally `iterations` and
/// `node_size`, which default to the crate's. An edge naming an unknown id is skipped. Returns
/// `{"positions": {"id": [x, y]}}`, the ids in order, or says what is wrong with the input.
pub fn force_layout(input: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(input)
        .map_err(|err| refuse(&format!("it does not read as JSON: {err}")))?;
    let node_list = value["nodes"]
        .as_array()
        .ok_or_else(|| refuse("nodes must be a list of ids"))?;
    let ids: Vec<&str> = node_list
        .iter()
        .map(|id| {
            id.as_str()
                .ok_or_else(|| refuse("nodes must be a list of ids"))
        })
        .collect::<Result<_, _>>()?;
    // A node id given twice keeps its first place, as the graph layout does.
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut unique: Vec<&str> = Vec::new();
    for id in ids {
        if !index.contains_key(id) {
            index.insert(id, unique.len());
            unique.push(id);
        }
    }
    let edge_list = value["edges"]
        .as_array()
        .ok_or_else(|| refuse("edges must be a list of [from, to] pairs"))?;
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for link in edge_list {
        let pair = link
            .as_array()
            .filter(|pair| pair.len() == 2)
            .ok_or_else(|| refuse("edges must be a list of [from, to] pairs"))?;
        let (Some(from), Some(to)) = (pair[0].as_str(), pair[1].as_str()) else {
            return Err(refuse("edges must be a list of [from, to] pairs"));
        };
        if let (Some(&from), Some(&to)) = (index.get(from), index.get(to)) {
            edges.push((from, to));
        }
    }
    let width = value["width"]
        .as_f64()
        .ok_or_else(|| refuse("width must be a number"))? as f32;
    let height = value["height"]
        .as_f64()
        .ok_or_else(|| refuse("height must be a number"))? as f32;
    let iterations = match value.get("iterations") {
        None | Some(Value::Null) => force::DEFAULT_ITERATIONS,
        Some(steps) => steps
            .as_u64()
            .and_then(|steps| u32::try_from(steps).ok())
            .ok_or_else(|| refuse("iterations must be a whole number"))?,
    };
    let node_size = match value.get("node_size") {
        None | Some(Value::Null) => force::DEFAULT_NODE_SIZE,
        Some(size) => size
            .as_f64()
            .ok_or_else(|| refuse("node_size must be a number"))? as f32,
    };

    let opts = ForceOptions {
        width,
        height,
        iterations,
        node_size,
    };
    let points = force::layout(unique.len(), &edges, &opts);
    let positions: BTreeMap<&str, [f32; 2]> = unique
        .iter()
        .zip(points)
        .map(|(&id, (x, y))| (id, [x, y]))
        .collect();
    serde_json::to_string(&json!({ "positions": positions })).map_err(|err| err.to_string())
}

/// The refusal for a force layout input, with the reason.
fn refuse(why: &str) -> String {
    format!("the force layout input does not read: {why}")
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

    #[test]
    fn the_force_layout_reads_ids_and_writes_positions() {
        let input = json!({
            "nodes": ["a", "b", "c"],
            "edges": [["a", "b"], ["b", "zz"]],
            "width": 400,
            "height": 300
        })
        .to_string();
        let out: Value =
            serde_json::from_str(&force_layout(&input).expect("a layout")).expect("JSON");
        assert_eq!(
            out["positions"].as_object().expect("positions by id").len(),
            3
        );
        for id in ["a", "b", "c"] {
            let x = out["positions"][id][0].as_f64().expect("x");
            let y = out["positions"][id][1].as_f64().expect("y");
            assert!(
                (28.0..=372.0).contains(&x) && (28.0..=272.0).contains(&y),
                "{id} is at {x}, {y}"
            );
        }
        assert_eq!(force_layout(&input), force_layout(&input));
    }

    #[test]
    fn bad_force_input_is_refused_with_why() {
        assert!(
            force_layout("not json")
                .unwrap_err()
                .starts_with("the force layout input does not read")
        );
        let no_width = json!({"nodes": [], "edges": []}).to_string();
        assert_eq!(
            force_layout(&no_width).unwrap_err(),
            "the force layout input does not read: width must be a number"
        );
    }
}
