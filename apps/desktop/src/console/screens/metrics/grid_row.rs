//! One row of the panel grid, laid out as the web's CSS grid lays it out: twelve columns with a gap, each cell as
//! wide as its span, and every cell as tall as the row's tallest (`align-items: stretch`). iced's rows cannot stretch
//! a cell to its neighbours' height inside a scrolled column, so the row lays its cells out twice: once to measure,
//! once at the row's height.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Clipboard, Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// The grid's columns.
const COLUMNS: u32 = 12;
/// Cells lay out as in the scrolled column they sit in: a fill height inside a cell counts as shrink.
const SHRINK_Y: Size<bool> = Size::new(false, true);

/// A grid row: cells with their spans (in twelfths), the gap between columns.
pub struct GridRow<'a, Message, Theme, Renderer> {
    cells: Vec<Element<'a, Message, Theme, Renderer>>,
    spans: Vec<u32>,
    columns: u32,
    gap: f32,
}

impl<'a, Message, Theme, Renderer> GridRow<'a, Message, Theme, Renderer> {
    /// A row of `cells`, each `(element, span)`; spans past twelve in all are cut at the row's end.
    pub fn new(cells: Vec<(Element<'a, Message, Theme, Renderer>, u32)>, gap: f32) -> Self {
        let (cells, spans) = cells
            .into_iter()
            .map(|(c, s)| (c, s.clamp(1, COLUMNS)))
            .unzip();
        Self {
            cells,
            spans,
            columns: COLUMNS,
            gap,
        }
    }

    /// A row of `columns` equal columns, one cell in each from the left (a short last row of tiles keeps its empty
    /// columns), every cell as tall as the tallest.
    pub fn even(
        cells: Vec<Element<'a, Message, Theme, Renderer>>,
        columns: usize,
        gap: f32,
    ) -> Self {
        let columns = (columns.max(cells.len()).max(1)) as u32;
        let spans = vec![1; cells.len()];
        Self {
            cells,
            spans,
            columns,
            gap,
        }
    }
}

/// Each cell's left edge and width in a row `width` wide of `columns` columns: a column is its share of the width
/// less the gaps, and a cell spans its columns and the gaps between them (as `grid-template-columns: repeat(12, 1fr)`
/// with `gap`).
pub fn cell_boxes(spans: &[u32], columns: u32, width: f32, gap: f32) -> Vec<(f32, f32)> {
    let column = ((width - gap * (columns - 1) as f32) / columns as f32).max(0.0);
    let mut start = 0u32;
    spans
        .iter()
        .map(|&span| {
            let span = span.min(columns.saturating_sub(start)).max(1);
            let x = start as f32 * (column + gap);
            let w = span as f32 * column + (span - 1) as f32 * gap;
            start = (start + span).min(columns);
            (x, w)
        })
        .collect()
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for GridRow<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn children(&self) -> Vec<Tree> {
        self.cells.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.cells);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let width = limits.max().width;
        let boxes = cell_boxes(&self.spans, self.columns, width, self.gap);
        // Measure: each cell at its width, as tall as it needs.
        let mut height = 0.0f32;
        for ((cell, tree), &(_, w)) in self.cells.iter_mut().zip(&mut tree.children).zip(&boxes) {
            let free = layout::Limits::with_compression(
                Size::new(w, 0.0),
                Size::new(w, f32::INFINITY),
                SHRINK_Y,
            );
            height = height.max(
                cell.as_widget_mut()
                    .layout(tree, renderer, &free)
                    .size()
                    .height,
            );
        }
        // Lay out again at the row's height: only the minimum rises. The maximum stays as it was, so the text inside
        // lays out with the same bounds and keeps its paragraphs (iced's drawn text holds them weakly, and a resized
        // paragraph would drop the text of the frame on screen).
        let nodes = self
            .cells
            .iter_mut()
            .zip(&mut tree.children)
            .zip(&boxes)
            .map(|((cell, tree), &(x, w))| {
                let exact = layout::Limits::with_compression(
                    Size::new(w, height),
                    Size::new(w, f32::INFINITY),
                    SHRINK_Y,
                );
                cell.as_widget_mut()
                    .layout(tree, renderer, &exact)
                    .move_to(Point::new(x, 0.0))
            })
            .collect();
        layout::Node::with_children(Size::new(width, height), nodes)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((cell, tree), layout) in self
                .cells
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
            {
                cell.as_widget_mut()
                    .operate(tree, layout, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for ((cell, tree), layout) in self
            .cells
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            cell.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.cells
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((cell, tree), layout)| {
                cell.as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((cell, tree), layout) in self
            .cells
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| layout.bounds().intersects(viewport))
        {
            cell.as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(
            &mut self.cells,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<GridRow<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(row: GridRow<'a, Message, Theme, Renderer>) -> Self {
        Element::new(row)
    }
}

#[cfg(test)]
mod tests {
    use super::cell_boxes;

    #[test]
    fn cells_take_their_columns_and_the_gaps_between_them() {
        // 12 columns of 50 with 11 gaps of 10: 710 wide.
        let b = cell_boxes(&[8, 4], 12, 710.0, 10.0);
        assert_eq!(
            b,
            vec![
                (0.0, 8.0 * 50.0 + 7.0 * 10.0),
                (8.0 * 60.0, 4.0 * 50.0 + 3.0 * 10.0)
            ]
        );
        // A row that is not full keeps its empty columns.
        let half = cell_boxes(&[6], 12, 710.0, 10.0);
        assert_eq!(half, vec![(0.0, 350.0)]);
    }

    #[test]
    fn even_columns_split_the_width() {
        // Three columns of 100 with gaps of 8; two tiles take the first two.
        assert_eq!(
            cell_boxes(&[1, 1], 3, 316.0, 8.0),
            vec![(0.0, 100.0), (108.0, 100.0)]
        );
    }

    #[test]
    fn spans_past_the_row_end_are_cut() {
        let b = cell_boxes(&[8, 8], 12, 710.0, 10.0);
        assert_eq!(b[1], (480.0, 4.0 * 50.0 + 3.0 * 10.0));
    }
}
