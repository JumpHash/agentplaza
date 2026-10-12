//! The floor in iced: the shader widget's program (pointer input → orbit, hover, pick), and the
//! overlay — name labels over the robots, the agent card, the legend — in the console's look.

use super::Floor;

/// A name label to draw over the scene, in widget pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub cid: u64,
    pub name: String,
    pub role: String,
    /// The second line: what it is doing.
    pub line: String,
    pub hue: f32,
    pub x: f32,
    pub y: f32,
    pub working: bool,
    pub highlighted: bool,
}

impl Floor {
    /// The labels for a `width` × `height` view (`updateLabels`).
    pub fn labels(&self, width: f32, height: f32) -> Vec<Label> {
        let _ = (width, height);
        Vec::new() // contract: W3
    }

    /// The robot under the pointer (`pick`).
    pub fn pick(&self, x: f32, y: f32, width: f32, height: f32) -> Option<u64> {
        let _ = (x, y, width, height);
        None // contract: W3
    }
}
