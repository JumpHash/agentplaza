//! `floor3d/rooms/kit.ts`: what every district room is built from — the style type, the
//! cartoon materials (`Kit`), floor textures and wall looks, and `Room`'s ready-made pieces.
//! Local frame: floor y = 0, back wall along z = -s, open front (z = +s) toward the plaza, the
//! middle (|x| < 1.3, |z| < 1.3) kept clear for the consoles.

use super::super::props::{Anim, Box2};
use super::super::scene::{Color, Mat, Obj, Tex};
use super::super::textures::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorKind { Planks, Parquet, Checker, Hazard, Slate, Carpet, Blueprint, Stars, Marble, Gravel, Tiles, Lawn }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallKind { Panel, Brick, Hedge, Glass, Fence, Stone, None }

/// A room style: its sign, size (half-width or radius, 3–5), shape, floor, walls, furnishing.
pub struct RoomStyle {
    pub name: &'static str,
    pub size: f32,
    pub round: bool,
    pub floor: FloorKind,
    pub wall: WallKind,
    pub build: fn(&mut Room),
}

/// The colours the kit tints from.
#[derive(Debug, Clone, Copy)]
pub struct KitPalette {
    pub sheet: Color,
    pub rule: Color,
    pub ink3: Color,
    pub signal: Color,
    pub ok: Color,
    pub info: Color,
}

/// Cartoon materials, some tinted with the room's hue (`Kit` in kit.ts).
#[derive(Clone)]
pub struct Kit {
    pub wood: Mat, pub dark_wood: Mat, pub metal: Mat, pub dark: Mat,
    pub white: Mat, pub paper: Mat, pub clay: Mat, pub leaf: Mat,
    pub leaf2: Mat, pub crate_: Mat, pub gold: Mat, pub red: Mat,
    pub blue: Mat, pub yellow: Mat, pub green: Mat, pub purple: Mat,
    pub accent: Mat, pub accent_soft: Mat,
    pub signal: Mat, pub ok: Mat, pub info: Mat,
    /// Monitor faces: lit while an agent of the room works.
    pub screen: Mat,
    /// Unlit glowing bits (basic materials).
    pub glow_ok: Mat, pub glow_info: Mat, pub glow_warm: Mat, pub glow_red: Mat,
}

impl Kit {
    /// `k.mat(color)`: any other toon colour, cached per kit.
    pub fn mat(&self, color: Color) -> Mat {
        Mat::toon(color) // contract: K (cache)
    }
}

/// `makeKit(hue, palette)`; hue in degrees.
pub fn make_kit(hue: f32, palette: &KitPalette) -> Kit {
    let _ = (hue, palette);
    let m = || Mat::toon(Color::WHITE);
    Kit {
        wood: m(), dark_wood: m(), metal: m(), dark: m(), white: m(), paper: m(), clay: m(), leaf: m(), leaf2: m(),
        crate_: m(), gold: m(), red: m(), blue: m(), yellow: m(), green: m(), purple: m(), accent: m(), accent_soft: m(),
        signal: m(), ok: m(), info: m(), screen: m(), glow_ok: m(), glow_info: m(), glow_warm: m(), glow_red: m(),
    } // contract: K
}

/// `floorTexture(kind, hue, size)`.
pub fn floor_texture(kind: FloorKind, hue: f32, size: f32) -> Tex {
    let _ = (kind, hue, size);
    super::super::textures::grass() // contract: K
}

/// `wallLook(kind, hue)`: the wall material, height and thickness (None for `WallKind::None`).
pub fn wall_look(kind: WallKind, hue: f32) -> Option<(Mat, f32, f32)> {
    let _ = (kind, hue);
    None // contract: K
}

/// A room being built (`Room` in kit.ts). `g` is its group; pieces are added under it.
pub struct Room {
    pub g: Obj,
    pub s: f32,
    pub hue: f32,
    pub k: Kit,
    pub rand: Rng,
    pub anim: Vec<Anim>,
    /// Each item's ground footprint in the room's local frame.
    pub boxes: Vec<Box2>,
}

impl Room {
    pub fn new(g: Obj, s: f32, hue: f32, k: Kit, rand: Rng) -> Room {
        Room { g, s, hue, k, rand, anim: Vec::new(), boxes: Vec::new() }
    }
    /// `item(x, z, rotY)`: a furniture group at (x, z), with its own footprint.
    pub fn item(&mut self, x: f32, z: f32, rot_y: f32) -> Obj { let _ = (x, z, rot_y); Obj::group() } // contract: K
    /// `box(parent, w, h, d, mat, x, y, z)`: a box standing on y (its base at y).
    #[allow(clippy::too_many_arguments)]
    pub fn box_(&mut self, parent: &Obj, w: f32, h: f32, d: f32, mat: &Mat, x: f32, y: f32, z: f32) -> Obj { let _ = (parent, w, h, d, mat, x, y, z); Obj::group() } // contract: K
    /// `cyl(parent, rTop, rBottom, h, mat, x, y, z, seg)`.
    #[allow(clippy::too_many_arguments)]
    pub fn cyl(&mut self, parent: &Obj, r_top: f32, r_bottom: f32, h: f32, mat: &Mat, x: f32, y: f32, z: f32, seg: u32) -> Obj { let _ = (parent, r_top, r_bottom, h, mat, x, y, z, seg); Obj::group() } // contract: K
    /// `ball(parent, r, mat, x, y, z)`.
    pub fn ball(&mut self, parent: &Obj, r: f32, mat: &Mat, x: f32, y: f32, z: f32) -> Obj { let _ = (parent, r, mat, x, y, z); Obj::group() } // contract: K
    /// `cone(parent, r, h, mat, x, y, z, seg)`.
    #[allow(clippy::too_many_arguments)]
    pub fn cone(&mut self, parent: &Obj, r: f32, h: f32, mat: &Mat, x: f32, y: f32, z: f32, seg: u32) -> Obj { let _ = (parent, r, h, mat, x, y, z, seg); Obj::group() } // contract: K
    /// `rug(w, d, mat, x, z, round)`.
    pub fn rug(&mut self, w: f32, d: f32, mat: &Mat, x: f32, z: f32, round: bool) -> Obj { let _ = (w, d, mat, x, z, round); Obj::group() } // contract: K
    pub fn desk(&mut self, x: f32, z: f32, rot_y: f32) -> Obj { let _ = (x, z, rot_y); Obj::group() } // contract: K
    /// `chair(x, z, rotY, parent?)`.
    pub fn chair(&mut self, x: f32, z: f32, rot_y: f32, parent: Option<&Obj>) -> Obj { let _ = (x, z, rot_y, parent); Obj::group() } // contract: K
    pub fn shelf(&mut self, x: f32, z: f32, rot_y: f32, w: f32, h: f32) -> Obj { let _ = (x, z, rot_y, w, h); Obj::group() } // contract: K
    pub fn plant(&mut self, x: f32, z: f32, scale: f32) -> Obj { let _ = (x, z, scale); Obj::group() } // contract: K
    pub fn crate_(&mut self, x: f32, z: f32, size: f32, rot_y: f32) -> Obj { let _ = (x, z, size, rot_y); Obj::group() } // contract: K
    pub fn lamp(&mut self, x: f32, z: f32) -> Obj { let _ = (x, z); Obj::group() } // contract: K
}
