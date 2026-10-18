//! The task graph's layout: where each task's card goes and how each link between two tasks is
//! drawn, for the Work screen. The web runs it as wasm (crates/apz-graph-web) and the desktop
//! natively, so both draw the same picture.
//!
//! The input is a project's graph as the hub's read model gives it (`GET /v1/projects/{id}/graph`):
//! its tasks, and its links as `[from, to]` edges where `to` runs after `from`. Fields the layout does
//! not use are ignored, so the read model goes in as it is. The output is a card per task and a route
//! per link, for the card size and the gaps in [`Options`]:
//!
//! - **Columns.** A task's column is the longest run of links behind it (longest-path layering), so
//!   every task stands to the right of everything it runs after.
//! - **Order.** Each column's cards are ordered by the mean height of the tasks they run after (their
//!   barycenter), ties in the input's order, and each column is centred on the tallest. The same graph
//!   always lays out the same way, so cards never move while a graph is unchanged.
//! - **Routes.** A route leaves its source card's right side and enters its target card's left side,
//!   in right angles. Between neighbouring columns it bends once, in the gap. A route that skips
//!   columns keeps to its source's row when that row is clear of the cards it passes, else to its
//!   target's row, else it runs in the channel above or below them. No route crosses a card.
//! - **Tracks.** The vertical runs in a gap share its middle while they only split from one source
//!   or merge into one target. Runs that would overlap otherwise, hiding which route goes where, get
//!   tracks of their own, spread across the gap and ordered to cross the fewest runs.
//!
//! A link to a task that is not in the graph, a repeated link and a task linked to itself are left
//! out. Links that close a cycle (the hub refuses them) are cut where the layering meets them and not
//! drawn, so a layout always ends.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A project's graph, as the read model gives it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Graph {
    /// The tasks, in the order a tie in a column keeps.
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// Each link as `[from, to]`: `to` runs after `from`.
    #[serde(default)]
    pub edges: Vec<[String; 2]>,
}

/// A task of the graph. Only its id matters to the layout.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Task {
    pub id: String,
}

/// The card size and the gaps, in the console's units (CSS pixels on the web).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Options {
    pub card_width: f64,
    pub card_height: f64,
    /// Between two columns: where routes bend.
    pub gap_x: f64,
    /// Between two cards of a column.
    pub gap_y: f64,
    /// Around the whole layout. At least half of `gap_y`, so a channel above or below the cards
    /// stays inside the layout.
    pub pad: f64,
}

impl Options {
    /// Whether the sizes can be laid out: cards of some size, gaps that are not negative, and padding
    /// that holds a channel (at least half of `gap_y`). The reason when they cannot.
    pub fn check(&self) -> Result<(), String> {
        let sizes = [
            ("card_width", self.card_width),
            ("card_height", self.card_height),
        ];
        for (name, value) in sizes {
            if !(value.is_finite() && value > 0.0) {
                return Err(format!("{name} must be more than 0"));
            }
        }
        let gaps = [
            ("gap_x", self.gap_x),
            ("gap_y", self.gap_y),
            ("pad", self.pad),
        ];
        for (name, value) in gaps {
            if !(value.is_finite() && value >= 0.0) {
                return Err(format!("{name} must be 0 or more"));
            }
        }
        if self.pad < self.gap_y / 2.0 {
            return Err(format!(
                "pad must be at least half of gap_y ({}), so a channel stays inside the layout",
                self.gap_y / 2.0
            ));
        }
        Ok(())
    }
}

impl Default for Options {
    /// The sizes for a caller that passes none, and for the fixtures that name none. The Work screens pass their own:
    /// the web's `CARD` (apps/web/src/views/work.ts) and the desktop's `work::CARD`, which a desktop test holds equal.
    fn default() -> Self {
        Self {
            card_width: 232.0,
            card_height: 104.0,
            gap_x: 46.0,
            gap_y: 14.0,
            pad: 24.0,
        }
    }
}

/// Where every card goes and how every link is drawn.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Layout {
    /// The layout's size, padding included.
    pub width: f64,
    pub height: f64,
    /// The task ids of each column, top to bottom.
    pub columns: Vec<Vec<String>>,
    /// A card per task, column by column, top to bottom.
    pub cards: Vec<Card>,
    /// A route per drawn link, in the input's order.
    pub routes: Vec<Route>,
}

/// One task's card.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Card {
    pub id: String,
    pub column: usize,
    pub row: usize,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// How one link is drawn.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Route {
    pub from: String,
    pub to: String,
    /// The corners, from the middle of the source card's right side to the middle of the target
    /// card's left side. Each step is horizontal or vertical.
    pub points: Vec<[f64; 2]>,
}

/// A card's rectangle.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Lays `graph` out with `options`.
pub fn layout(graph: &Graph, options: &Options) -> Layout {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut ids: Vec<&str> = Vec::new();
    for task in &graph.tasks {
        if !index.contains_key(task.id.as_str()) {
            index.insert(&task.id, ids.len());
            ids.push(&task.id);
        }
    }
    // The links the layout draws: known tasks, not a task after itself, each once.
    let mut links: Vec<(usize, usize)> = Vec::new();
    for [from, to] in &graph.edges {
        if let (Some(&from), Some(&to)) = (index.get(from.as_str()), index.get(to.as_str()))
            && from != to
            && !links.contains(&(from, to))
        {
            links.push((from, to));
        }
    }
    let mut upstream: Vec<Vec<usize>> = vec![Vec::new(); ids.len()];
    for &(from, to) in &links {
        upstream[to].push(from);
    }
    let layer = layers(&upstream);

    let count = layer.iter().max().map_or(0, |deepest| deepest + 1);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (task, &at) in layer.iter().enumerate() {
        columns[at].push(task);
    }
    let Options {
        card_width: w,
        card_height: h,
        gap_x,
        gap_y,
        pad,
    } = *options;
    let column_height = |cards: usize| {
        if cards == 0 {
            0.0
        } else {
            cards as f64 * h + (cards - 1) as f64 * gap_y
        }
    };
    let tallest = columns
        .iter()
        .map(|column| column_height(column.len()))
        .fold(0.0, f64::max);

    let mut rects: Vec<Rect> = vec![
        Rect {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
        };
        ids.len()
    ];
    let mut rows = vec![0; ids.len()];
    for (at, column) in columns.iter_mut().enumerate() {
        if at > 0 {
            // The barycenter: the mean height of the tasks it runs after, in earlier columns.
            let center = |task: usize| {
                let ys: Vec<f64> = upstream[task]
                    .iter()
                    .filter(|&&from| layer[from] < at)
                    .map(|&from| rects[from].y + h / 2.0)
                    .collect();
                if ys.is_empty() {
                    f64::INFINITY
                } else {
                    ys.iter().sum::<f64>() / ys.len() as f64
                }
            };
            let mut keyed: Vec<(f64, usize)> =
                column.iter().map(|&task| (center(task), task)).collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            *column = keyed.into_iter().map(|(_, task)| task).collect();
        }
        let x = pad + at as f64 * (w + gap_x);
        let top = pad + (tallest - column_height(column.len())) / 2.0;
        for (row, &task) in column.iter().enumerate() {
            rects[task] = Rect {
                x,
                y: top + row as f64 * (h + gap_y),
                width: w,
                height: h,
            };
            rows[task] = row;
        }
    }

    // A link the layering cut (it closed a cycle) runs backwards, and is not drawn.
    let drawn: Vec<(usize, usize)> = links
        .iter()
        .copied()
        .filter(|&(from, to)| layer[to] > layer[from])
        .collect();
    let mut runs: Vec<Run> = Vec::new();
    let plans: Vec<Vec<Corner>> = drawn
        .iter()
        .map(|&(from, to)| {
            let between: Vec<Rect> = columns[layer[from] + 1..layer[to]]
                .iter()
                .flatten()
                .map(|&task| rects[task])
                .collect();
            let ends = Ends {
                from,
                to,
                first_gap: layer[from],
                last_gap: layer[to] - 1,
            };
            plan(&ends, rects[from], rects[to], &between, gap_y, &mut runs)
        })
        .collect();
    let xs = tracks(&runs, count, options);
    let routes = drawn
        .iter()
        .zip(plans)
        .map(|(&(from, to), plan)| Route {
            from: ids[from].to_string(),
            to: ids[to].to_string(),
            points: corners(
                plan.into_iter()
                    .map(|corner| match corner {
                        Corner::At(point) => point,
                        Corner::Run(run, y) => [xs[run], y],
                    })
                    .collect(),
            ),
        })
        .collect();

    let width = if count == 0 {
        2.0 * pad
    } else {
        2.0 * pad + count as f64 * w + (count - 1) as f64 * gap_x
    };
    Layout {
        width,
        height: 2.0 * pad + tallest,
        columns: columns
            .iter()
            .map(|column| column.iter().map(|&task| ids[task].to_string()).collect())
            .collect(),
        cards: columns
            .iter()
            .enumerate()
            .flat_map(|(at, column)| column.iter().map(move |&task| (at, task)))
            .map(|(at, task)| Card {
                id: ids[task].to_string(),
                column: at,
                row: rows[task],
                x: rects[task].x,
                y: rects[task].y,
                width: w,
                height: h,
            })
            .collect(),
        routes,
    }
}

/// Each task's column: 0 with no links, else one more than the deepest task it runs after. A
/// cycle met on the way is cut where the walk finds it, so this always ends.
fn layers(upstream: &[Vec<usize>]) -> Vec<usize> {
    let mut layer: Vec<Option<usize>> = vec![None; upstream.len()];
    let mut walking = vec![false; upstream.len()];
    for start in 0..upstream.len() {
        let mut stack = vec![start];
        while let Some(&task) = stack.last() {
            if layer[task].is_some() {
                stack.pop();
                continue;
            }
            walking[task] = true;
            let pending: Vec<usize> = upstream[task]
                .iter()
                .copied()
                .filter(|&from| layer[from].is_none() && !walking[from])
                .collect();
            if pending.is_empty() {
                layer[task] = Some(
                    upstream[task]
                        .iter()
                        .filter_map(|&from| layer[from])
                        .map(|at| at + 1)
                        .max()
                        .unwrap_or(0),
                );
                walking[task] = false;
                stack.pop();
            } else {
                stack.extend(pending);
            }
        }
    }
    layer.into_iter().map(Option::unwrap_or_default).collect()
}

/// A route's tasks, and the gaps it leaves its source's column by and enters its target's by.
struct Ends {
    from: usize,
    to: usize,
    first_gap: usize,
    last_gap: usize,
}

/// A corner of a planned route: a fixed point, or the point at height `y` on one of the route's
/// vertical runs, whose x is its run's track.
enum Corner {
    At([f64; 2]),
    Run(usize, f64),
}

/// A route's vertical run in the gap after column `gap`, from `lo` to `hi`: reached from the left
/// at height `left`, and left to the right at height `right`.
struct Run {
    gap: usize,
    from: usize,
    to: usize,
    lo: f64,
    hi: f64,
    left: f64,
    right: f64,
}

/// The corners of the route `ends` names, from card `source` to card `target` (in a later
/// column), past the cards of the columns `between` them. Its vertical runs go into `runs`, for
/// [`tracks`] to place.
fn plan(
    ends: &Ends,
    source: Rect,
    target: Rect,
    between: &[Rect],
    gap_y: f64,
    runs: &mut Vec<Run>,
) -> Vec<Corner> {
    let start = [source.x + source.width, source.y + source.height / 2.0];
    let end = [target.x, target.y + target.height / 2.0];
    let (y1, y2) = (start[1], end[1]);
    let margin = gap_y / 2.0;
    let clear = |y: f64| {
        between
            .iter()
            .all(|card| y < card.y - margin || y > card.y + card.height + margin)
    };
    let mut run = |gap: usize, left: f64, right: f64| {
        runs.push(Run {
            gap,
            from: ends.from,
            to: ends.to,
            lo: left.min(right),
            hi: left.max(right),
            left,
            right,
        });
        runs.len() - 1
    };
    if between.is_empty() || clear(y1) || clear(y2) {
        if y1 == y2 {
            return vec![Corner::At(start), Corner::At(end)];
        }
        // Neighbours bend in their one gap. A route that skips columns keeps to its source's row
        // when that is clear and bends before its target, else bends after its source and keeps to
        // its target's row.
        let gap = if !between.is_empty() && clear(y1) {
            ends.last_gap
        } else {
            ends.first_gap
        };
        let at = run(gap, y1, y2);
        return vec![
            Corner::At(start),
            Corner::Run(at, y1),
            Corner::Run(at, y2),
            Corner::At(end),
        ];
    }
    // Both rows are blocked: the channel above or below the cards between, whichever is nearer.
    let top = between
        .iter()
        .map(|card| card.y)
        .fold(f64::INFINITY, f64::min)
        - margin;
    let bottom = between
        .iter()
        .map(|card| card.y + card.height)
        .fold(f64::NEG_INFINITY, f64::max)
        + margin;
    let channel = if (top - y1).abs() <= (bottom - y1).abs() {
        top
    } else {
        bottom
    };
    let out = run(ends.first_gap, y1, channel);
    let into = run(ends.last_gap, channel, y2);
    vec![
        Corner::At(start),
        Corner::Run(out, y1),
        Corner::Run(out, channel),
        Corner::Run(into, channel),
        Corner::Run(into, y2),
        Corner::At(end),
    ]
}

/// The x of each run. Each gap's runs go on tracks spread evenly across the gap. Runs that overlap
/// share a track only when their routes share a source or a target, which draws as a split or a
/// merge; any other overlap would hide which route goes where, so those runs get tracks of their
/// own. The tracks are ordered so their stubs cross the fewest runs.
fn tracks(runs: &[Run], columns: usize, options: &Options) -> Vec<f64> {
    let mut xs = vec![0.0; runs.len()];
    let mut track = vec![0; runs.len()];
    for gap in 0..columns.saturating_sub(1) {
        let mut members: Vec<usize> = (0..runs.len()).filter(|&at| runs[at].gap == gap).collect();
        if members.is_empty() {
            continue;
        }
        members.sort_by(|&a, &b| {
            runs[a]
                .lo
                .total_cmp(&runs[b].lo)
                .then(runs[a].hi.total_cmp(&runs[b].hi))
                .then(a.cmp(&b))
        });
        let mut count = 0;
        for (placed, &at) in members.iter().enumerate() {
            let taken: Vec<usize> = members[..placed]
                .iter()
                .filter(|&&other| clash(&runs[at], &runs[other]))
                .map(|&other| track[other])
                .collect();
            let free = (0..=count).find(|t| !taken.contains(t)).unwrap_or(count);
            track[at] = free;
            count = count.max(free + 1);
        }
        let slot = best_order(runs, &members, &track, count);
        let left =
            options.pad + gap as f64 * (options.card_width + options.gap_x) + options.card_width;
        for &at in &members {
            xs[at] = left + options.gap_x * (slot[track[at]] + 1) as f64 / (count + 1) as f64;
        }
    }
    xs
}

/// Whether two runs of one gap may not share a track: they overlap, and their routes share neither
/// a source nor a target.
fn clash(a: &Run, b: &Run) -> bool {
    a.lo < b.hi && b.lo < a.hi && a.from != b.from && a.to != b.to
}

/// The slot, left to right, of each of a gap's `count` tracks that makes the fewest crossings: a
/// run's stub to the left crosses the runs on tracks left of its own, and its stub to the right the
/// runs on tracks right of it, where their span holds the stub's height. Every order is tried up to
/// six tracks; past that the tracks keep the order they were found in. A tie keeps the first order
/// tried.
fn best_order(runs: &[Run], members: &[usize], track: &[usize], count: usize) -> Vec<usize> {
    let identity: Vec<usize> = (0..count).collect();
    if count <= 1 || count > 6 {
        return identity;
    }
    let crossings = |slot: &[usize]| {
        let mut crossed = 0;
        for &a in members {
            for &b in members {
                let (across, stub) = (&runs[a], &runs[b]);
                if track[a] == track[b] {
                    continue;
                }
                let holds = |y: f64| across.lo < y && y < across.hi;
                if slot[track[a]] < slot[track[b]] && holds(stub.left) {
                    crossed += 1;
                }
                if slot[track[a]] > slot[track[b]] && holds(stub.right) {
                    crossed += 1;
                }
            }
        }
        crossed
    };
    let mut best = identity.clone();
    let mut fewest = crossings(&best);
    let mut slot = identity;
    while next_permutation(&mut slot) {
        let crossed = crossings(&slot);
        if crossed < fewest {
            fewest = crossed;
            best = slot.clone();
        }
    }
    best
}

/// Steps `items` to its next permutation in lexicographic order; false after the last.
fn next_permutation(items: &mut [usize]) -> bool {
    let Some(pivot) = (1..items.len()).rev().find(|&at| items[at - 1] < items[at]) else {
        return false;
    };
    let swap = (pivot..items.len())
        .rev()
        .find(|&at| items[at] > items[pivot - 1])
        .unwrap_or(pivot);
    items.swap(pivot - 1, swap);
    items[pivot..].reverse();
    true
}

/// The corners only: a point equal to the one before it, or in line with its neighbours, goes.
fn corners(points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    let mut kept: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for point in points {
        if kept.last() == Some(&point) {
            continue;
        }
        if kept.len() >= 2 {
            let [before, last] = [kept[kept.len() - 2], kept[kept.len() - 1]];
            let in_line = (before[0] == last[0] && last[0] == point[0])
                || (before[1] == last[1] && last[1] == point[1]);
            if in_line {
                kept.pop();
            }
        }
        kept.push(point);
    }
    kept
}

#[cfg(test)]
mod tests;
