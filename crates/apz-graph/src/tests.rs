//! The layout's tests: the golden fixtures in assets/graphs, and the rules every layout keeps (no
//! card overlaps another, no route crosses a card, every route is right-angled and joins its own
//! cards, every task stands right of what it runs after), on the fixtures and on generated graphs.

use std::path::PathBuf;

use serde_json::Value;

use super::*;

/// The fixtures: `{about, graph, options?, layout}` files in assets/graphs. With
/// `APZ_GRAPH_BLESS=1`, the golden test writes each fixture's `layout` from the code instead of
/// checking it; read the diff before keeping it.
fn fixtures() -> Vec<(PathBuf, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/graphs");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{}: {err}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no fixtures in {}", dir.display());
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("a fixture reads");
            let value = serde_json::from_str(&text)
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            (path, value)
        })
        .collect()
}

fn graph_of(fixture: &Value) -> Graph {
    serde_json::from_value(fixture["graph"].clone()).expect("the fixture's graph")
}

fn options_of(fixture: &Value) -> Options {
    match fixture.get("options") {
        Some(options) => serde_json::from_value(options.clone()).expect("the fixture's options"),
        None => Options::default(),
    }
}

#[test]
fn every_fixture_lays_out_as_its_golden_layout() {
    let bless = std::env::var("APZ_GRAPH_BLESS").is_ok_and(|value| value == "1");
    for (path, mut fixture) in fixtures() {
        let laid = layout(&graph_of(&fixture), &options_of(&fixture));
        if bless {
            fixture["layout"] = serde_json::to_value(&laid).expect("a layout serializes");
            let text = serde_json::to_string_pretty(&fixture).expect("a fixture serializes");
            std::fs::write(&path, text + "\n").expect("the fixture is written");
            continue;
        }
        let golden: Layout = serde_json::from_value(fixture["layout"].clone())
            .unwrap_or_else(|err| panic!("{}: no golden layout ({err})", path.display()));
        assert_eq!(laid, golden, "{}", path.display());
    }
}

#[test]
fn a_graph_lays_out_the_same_every_time() {
    for (path, fixture) in fixtures() {
        let graph = graph_of(&fixture);
        let options = options_of(&fixture);
        assert_eq!(
            layout(&graph, &options),
            layout(&graph, &options),
            "{}",
            path.display()
        );
    }
}

#[test]
fn every_fixture_keeps_the_rules() {
    for (path, fixture) in fixtures() {
        let graph = graph_of(&fixture);
        let laid = layout(&graph, &options_of(&fixture));
        if let Err(broken) = keeps_the_rules(&graph, &laid) {
            panic!("{}: {broken}", path.display());
        }
    }
}

#[test]
fn generated_graphs_keep_the_rules() {
    // A fixed seed: each run checks the same few hundred graphs.
    let mut random = Lcg(0x5eed_1234_abcd_0042);
    let options = [
        Options::default(),
        Options {
            card_width: 420.0,
            card_height: 206.0,
            gap_x: 70.0,
            gap_y: 24.0,
            pad: 12.0,
        },
    ];
    for case in 0..400 {
        let tasks = 1 + random.below(24);
        let density = 1 + random.below(5);
        let mut graph = Graph::default();
        for at in 0..tasks {
            graph.tasks.push(Task {
                id: format!("t{at}"),
            });
        }
        for to in 1..tasks {
            for from in 0..to {
                if random.below(12) < density {
                    graph.edges.push([format!("t{from}"), format!("t{to}")]);
                }
            }
        }
        // Posting order is not run order: shuffle the tasks, as a relinked plan can be.
        for at in (1..graph.tasks.len()).rev() {
            graph.tasks.swap(at, random.below(at + 1));
        }
        let laid = layout(&graph, &options[case % 2]);
        if let Err(broken) = keeps_the_rules(&graph, &laid) {
            panic!("generated graph {case}: {broken}\n{graph:?}");
        }
    }
}

#[test]
fn links_the_layout_cannot_draw_are_left_out() {
    let graph = Graph {
        tasks: ["a", "b", "c"]
            .into_iter()
            .map(|id| Task { id: id.to_string() })
            .collect(),
        edges: [
            ["a", "b"],
            ["a", "b"],
            ["b", "b"],
            ["a", "nope"],
            ["b", "c"],
            // Closes a cycle: cut, and not drawn.
            ["c", "a"],
        ]
        .into_iter()
        .map(|[from, to]| [from.to_string(), to.to_string()])
        .collect(),
    };
    let laid = layout(&graph, &Options::default());
    let drawn: Vec<(&str, &str)> = laid
        .routes
        .iter()
        .map(|route| (route.from.as_str(), route.to.as_str()))
        .collect();
    assert_eq!(drawn.len(), 2, "{drawn:?}");
    assert!(drawn.contains(&("b", "c")));
    assert_eq!(laid.cards.len(), 3, "every task has a card");
    keeps_the_rules(&graph, &laid).expect("the rules hold");
}

#[test]
fn an_empty_graph_is_its_padding() {
    let laid = layout(&Graph::default(), &Options::default());
    assert!(laid.cards.is_empty() && laid.routes.is_empty() && laid.columns.is_empty());
    assert_eq!((laid.width, laid.height), (48.0, 48.0));
}

#[test]
fn a_route_between_neighbours_bends_once_in_the_gap() {
    let graph: Graph = serde_json::from_value(serde_json::json!({
        "tasks": [{"id": "a"}, {"id": "b"}, {"id": "c"}],
        "edges": [["a", "b"], ["a", "c"]],
        "stage": "implementation"
    }))
    .expect("the read model's shape reads");
    let laid = layout(&graph, &Options::default());
    let route = |to: &str| {
        laid.routes
            .iter()
            .find(|route| route.to == to)
            .expect("a route")
            .points
            .clone()
    };
    // a is centred on b and c: each route steps out, bends in the gap's middle, and steps in.
    // a: x 24, y 83 (centred on the 222-tall column); b: y 24; c: y 142.
    let gap = 24.0 + 232.0 + 23.0;
    assert_eq!(
        route("b"),
        vec![[256.0, 135.0], [gap, 135.0], [gap, 76.0], [302.0, 76.0]]
    );
    assert_eq!(
        route("c"),
        vec![[256.0, 135.0], [gap, 135.0], [gap, 194.0], [302.0, 194.0]]
    );
}

/// A deterministic random source for the generated graphs.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound as u64) as usize
    }
}

/// The rules every layout keeps, or the first one it breaks.
fn keeps_the_rules(graph: &Graph, laid: &Layout) -> Result<(), String> {
    let card = |id: &str| {
        laid.cards
            .iter()
            .find(|card| card.id == id)
            .ok_or_else(|| format!("no card for {id}"))
    };
    let mut ids: Vec<&str> = graph.tasks.iter().map(|task| task.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if laid.cards.len() != ids.len() {
        return Err(format!(
            "{} cards for {} tasks",
            laid.cards.len(),
            ids.len()
        ));
    }
    for (at, a) in laid.cards.iter().enumerate() {
        if a.x < 0.0 || a.y < 0.0 || a.x + a.width > laid.width || a.y + a.height > laid.height {
            return Err(format!("{} is outside the layout", a.id));
        }
        for b in &laid.cards[at + 1..] {
            let apart = a.x + a.width <= b.x
                || b.x + b.width <= a.x
                || a.y + a.height <= b.y
                || b.y + b.height <= a.y;
            if !apart {
                return Err(format!("{} overlaps {}", a.id, b.id));
            }
        }
    }
    for route in &laid.routes {
        let (from, to) = (card(&route.from)?, card(&route.to)?);
        if from.column >= to.column {
            return Err(format!("{} does not stand right of {}", to.id, from.id));
        }
        let first = route.points.first().copied().unwrap_or_default();
        let last = route.points.last().copied().unwrap_or_default();
        if first != [from.x + from.width, from.y + from.height / 2.0] {
            return Err(format!(
                "{} → {} does not leave its source's right side",
                from.id, to.id
            ));
        }
        if last != [to.x, to.y + to.height / 2.0] {
            return Err(format!(
                "{} → {} does not enter its target's left side",
                from.id, to.id
            ));
        }
        for step in route.points.windows(2) {
            let ([x1, y1], [x2, y2]) = (step[0], step[1]);
            if (x1 != x2) == (y1 != y2) {
                return Err(format!(
                    "{} → {} has a step that is not a right angle",
                    from.id, to.id
                ));
            }
            if x1.min(x2) < 0.0
                || x1.max(x2) > laid.width
                || y1.min(y2) < 0.0
                || y1.max(y2) > laid.height
            {
                return Err(format!("{} → {} leaves the layout", from.id, to.id));
            }
            for other in &laid.cards {
                let inside_x = |x: f64| other.x < x && x < other.x + other.width;
                let inside_y = |y: f64| other.y < y && y < other.y + other.height;
                let crosses = if y1 == y2 {
                    inside_y(y1) && x1.min(x2) < other.x + other.width && x1.max(x2) > other.x
                } else {
                    inside_x(x1) && y1.min(y2) < other.y + other.height && y1.max(y2) > other.y
                };
                if crosses {
                    return Err(format!("{} → {} crosses {}", from.id, to.id, other.id));
                }
            }
        }
    }
    Ok(())
}
