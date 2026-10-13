//! Smoke test: the desktop links the board core (crates/apz-board) and builds a board from its embedded assets.

use std::collections::BTreeMap;

#[test]
fn board_core_builds_the_edge_board() {
    let board = apz_board::Board::from_kind("edge", &BTreeMap::new());
    assert!(!board.parts().is_empty(), "the edge board has resting items");
    let b = board.bounds();
    assert!(b.min.iter().chain(b.max.iter()).all(|v| v.is_finite()), "the bounds are finite");
    assert!(board.part_count() >= board.parts().len() / 2, "the part count is plausible");
}
