//! The robot rig, as `robots/rig.ts`. A species is baked once (its parts merged per bone, with the
//! per-vertex colour and slot the renderer's robot hues read), and each robot gets its own bone tree and
//! its own few materials. Meshes per robot: one matte per bone (its ink hull hangs on the same node, a
//! second draw, as the renderer draws a hull), one per solo material (eye, light, thruster, clear, inline)
//! and one pupil mesh per eye row.

use std::collections::BTreeMap;

use glam::{Mat4, Vec3};

use super::super::geometry::{from_triangles, transformed};
use super::super::robot::RobotPalette;
use super::super::scene::{Color, Geo as Mesh, Mat, Material, Obj, RobotHues, Shading};
use super::super::toon::toon_mat;
use super::geometry::{Piece, eye_geometries, hull_of, join, part_geometry};
use super::types::{InlineMat, MatRef, Part, Species, Tool};

/// A part smaller than this (its bounding radius) is not inked, as in `toon.ts`.
const INK_MIN: f32 = 0.03;

/// What a solo mesh draws with: a robot's own eye, light or thruster material, or one shared by the species.
#[derive(Clone)]
pub enum Bind {
    Eye,
    Light,
    Thruster,
    Own(Mat),
}

pub struct Solo {
    pub bind: Bind,
    pub geo: Mesh,
}

/// One bone's baked geometry, shared by every robot of the species.
#[derive(Default)]
pub struct BoneGeo {
    pub matte: Option<Mesh>,
    /// The ink line over the bone's inked parts.
    pub hull: Option<Mesh>,
    pub solo: Vec<Solo>,
    /// One pupil mesh per eye row, its pivot at its own height `y`.
    pub pupils: Vec<(f32, Mesh)>,
}

/// A baked species: the geometry of each bone (by name).
pub struct SpeciesGeo {
    pub bones: BTreeMap<String, BoneGeo>,
}

/// One robot's own materials: its hues on the matte, and its eye, light and thruster.
pub struct RobotMats {
    pub matte: Mat,
    pub eye: Mat,
    pub light: Mat,
    pub thruster: Mat,
}

/// The robot's paint: the matte takes the shell, shell2 and panel hues per vertex slot; the eye, light
/// and thruster take the palette. `hue` tints the panel, `body_hue` the shell (both in degrees).
pub fn robot_mats(pal: &RobotPalette, hue: f32, body_hue: f32) -> RobotMats {
    let hsl = |h: f32, s: f32, l: f32| Color::hsl(h / 360.0, s, l);
    RobotMats {
        matte: toon_mat(Material {
            color: Color::WHITE,
            robot: Some(RobotHues {
                shell: hsl(body_hue, 0.62, 0.62),
                shell2: hsl(body_hue, 0.6, 0.46),
                panel: hsl(hue, 0.55, 0.78),
            }),
            ..Material::default()
        }),
        eye: toon_mat(Material {
            color: pal.ok,
            emissive: pal.ok,
            emissive_intensity: 1.2,
            ..Material::default()
        }),
        light: toon_mat(Material {
            color: pal.signal,
            emissive: pal.signal,
            emissive_intensity: 1.2,
            ..Material::default()
        }),
        thruster: Mat::new(Material {
            shading: Shading::Basic,
            color: pal.info,
            ..Material::default()
        }),
    }
}

/// The clear material: translucent, not written to the depth buffer.
fn clear_mat() -> Mat {
    toon_mat(Material {
        transparent: true,
        opacity: 0.5,
        depth_write: false,
        ..Material::default()
    })
}

/// An inline material, as the web's: a colour, an optional glow (1 with an emissive colour, else 0), an opacity.
fn inline_mat(i: &InlineMat) -> Mat {
    let opacity = i.opacity.unwrap_or(1.0);
    let emissive = i.emissive.as_deref();
    toon_mat(Material {
        color: Color::css(&i.color).unwrap_or(Color::WHITE),
        emissive: emissive.and_then(Color::css).unwrap_or(Color::BLACK),
        emissive_intensity: i.glow.unwrap_or(if emissive.is_some() { 1.0 } else { 0.0 }),
        transparent: opacity < 1.0,
        opacity,
        depth_write: opacity >= 1.0,
        ..Material::default()
    })
}

/// A solo part's key: its material name, or the inline material's values.
fn solo_key(mat: &MatRef) -> String {
    match mat {
        MatRef::Name(n) => n.clone(),
        MatRef::Inline(i) => format!("inline:{i:?}"),
    }
}

fn solo_bind(mat: &MatRef) -> Bind {
    match mat {
        MatRef::Name(n) if n == "eye" => Bind::Eye,
        MatRef::Name(n) if n == "light" => Bind::Light,
        MatRef::Name(n) if n == "thruster" => Bind::Thruster,
        MatRef::Name(_) => Bind::Own(clear_mat()),
        MatRef::Inline(i) => Bind::Own(inline_mat(i)),
    }
}

/// The colour and slot of a matte part: the robot's three hues, or a fixed palette colour (glass is the
/// ink colour, in slot 4: it never glows).
fn matte_of(name: &str, pal: &RobotPalette, accent: Color) -> Option<([f32; 3], u8)> {
    let white = [1.0; 3];
    let fixed = |c: Color| c.to_array();
    Some(match name {
        "shell" => (white, 1),
        "shell2" => (white, 2),
        "panel" => (white, 3),
        "paper" => (fixed(pal.paper), 0),
        "trim" => (fixed(pal.rule), 0),
        "signal" => (fixed(pal.signal), 0),
        "accent" => (fixed(accent), 0),
        "ink" => (fixed(pal.ink), 0),
        "glass" => (fixed(pal.ink), 4),
        _ => return None,
    })
}

#[derive(Default)]
struct Acc {
    matte: Vec<Piece>,
    hull: Vec<Piece>,
    solo: BTreeMap<String, (Vec<Piece>, Bind)>,
    pupils: Vec<(f32, Vec<Piece>)>,
}

/// Bakes parts into the accumulators of their bones (`bone_of` names each part's bone).
fn bake(
    parts: &[Part],
    pal: &RobotPalette,
    accent: Color,
    accs: &mut BTreeMap<String, Acc>,
    bone_of: impl Fn(&Part) -> String,
) {
    for p in parts {
        let (geo, radius) = part_geometry(p);
        let acc = accs.entry(bone_of(p)).or_default();
        if let MatRef::Name(name) = &p.mat
            && let Some((colour, slot)) = matte_of(name, pal, accent)
        {
            // The ink line: every part but an ink one, unless `outline` says otherwise; small parts are not inked.
            if p.outline.unwrap_or(name != "ink") && radius >= INK_MIN {
                acc.hull.push(Piece::plain(geo.clone()));
            }
            acc.matte.push(Piece { geo, colour, slot });
            continue;
        }
        let entry = acc
            .solo
            .entry(solo_key(&p.mat))
            .or_insert_with(|| (Vec::new(), solo_bind(&p.mat)));
        entry.0.push(Piece::plain(geo));
    }
}

/// The eyes: the glass sockets join the matte (glass colour, slot 4), each pupil joins its row's mesh,
/// moved so the row's pivot is at its own height.
fn bake_eyes(sp: &Species, pal: &RobotPalette, accs: &mut BTreeMap<String, Acc>) {
    for e in &sp.eyes {
        let (pupil, glass) = eye_geometries(e);
        let acc = accs.entry(e.bone.clone()).or_default();
        if let Some(glass) = glass {
            acc.matte.push(Piece {
                geo: glass,
                colour: pal.ink.to_array(),
                slot: 4,
            });
        }
        let y = e.at[1];
        let pupil = Piece::plain(transformed(
            &pupil,
            Mat4::from_translation(Vec3::new(0.0, -y, 0.0)),
        ));
        match acc.pupils.iter_mut().find(|(row, _)| *row == y) {
            Some((_, list)) => list.push(pupil),
            None => acc.pupils.push((y, vec![pupil])),
        }
    }
}

fn finish(acc: Acc) -> BoneGeo {
    BoneGeo {
        matte: join(&acc.matte),
        hull: hull_of(&acc.hull),
        solo: acc
            .solo
            .into_iter()
            .filter_map(|(_, (pieces, bind))| join(&pieces).map(|geo| Solo { bind, geo }))
            .collect(),
        pupils: acc
            .pupils
            .into_iter()
            .filter_map(|(y, list)| join(&list).map(|g| (y, g)))
            .collect(),
    }
}

/// Bakes a species' geometry (once per species and orchestrator flag). The orchestrator also wears the
/// species' extra parts.
pub fn bake_species(sp: &Species, pal: &RobotPalette, orchestrator: bool) -> SpeciesGeo {
    let accent = Color::css(&sp.accent).unwrap_or(Color::WHITE);
    let mut accs = BTreeMap::new();
    bake(&sp.parts, pal, accent, &mut accs, |p| {
        p.bone.clone().unwrap_or_default()
    });
    if orchestrator && let Some(extra) = &sp.orchestrator {
        bake(&extra.parts, pal, accent, &mut accs, |p| {
            p.bone.clone().unwrap_or_default()
        });
    }
    bake_eyes(sp, pal, &mut accs);
    SpeciesGeo {
        bones: accs
            .into_iter()
            .map(|(name, acc)| (name, finish(acc)))
            .collect(),
    }
}

/// Bakes a tool or a carried item: its parts at its origin, in the signal accent (as the web's).
pub fn bake_tool(tool: &Tool, pal: &RobotPalette) -> BoneGeo {
    let mut accs = BTreeMap::new();
    bake(&tool.parts, pal, pal.signal, &mut accs, |_| {
        "root".to_string()
    });
    accs.remove("root").map_or_else(BoneGeo::default, finish)
}

/// Bakes a station prop's parts (no bones: a part without one goes on the root) in `accent`, as the web's
/// `bakeSpecies` does for a prop built as a one-bone species (robots/props.ts `buildProp`).
pub fn bake_prop(parts: &[Part], pal: &RobotPalette, accent: Color) -> BoneGeo {
    let mut accs = BTreeMap::new();
    bake(parts, pal, accent, &mut accs, |p| {
        p.bone.clone().unwrap_or_else(|| "root".to_string())
    });
    accs.remove("root").map_or_else(BoneGeo::default, finish)
}

/// The meshes of one bone's geometry under `parent`: the matte (its hull on the same node), the solos,
/// and the pupils (pushed to `pupils`, so the robot can blink them).
pub fn add_bone(parent: &Obj, bg: &BoneGeo, mats: &RobotMats, pupils: &mut Vec<Obj>) {
    if bg.matte.is_some() || bg.hull.is_some() {
        // A bone with only inked parts still gets its hull (on an empty mesh).
        let geo = bg
            .matte
            .clone()
            .unwrap_or_else(|| from_triangles(Vec::new()));
        let m = mesh(&geo, &mats.matte, true);
        m.get().hull = bg.hull.clone();
        parent.add(&m);
    }
    for s in &bg.solo {
        let (mat, cast) = match &s.bind {
            Bind::Eye => (mats.eye.clone(), false),
            Bind::Light => (mats.light.clone(), false),
            Bind::Thruster => (mats.thruster.clone(), true),
            Bind::Own(m) => (m.clone(), true),
        };
        parent.add(&mesh(&s.geo, &mat, cast));
    }
    for (y, geo) in &bg.pupils {
        let m = mesh(geo, &mats.eye, false);
        m.set_position(0.0, *y, 0.0);
        parent.add(&m);
        pupils.push(m);
    }
}

fn mesh(geo: &Mesh, mat: &Mat, shadows: bool) -> Obj {
    let m = Obj::mesh(geo, mat);
    m.set_shadows(shadows, shadows);
    m
}

/// A robot's instance of a baked species: its bone groups (each at its `at`, under its parent), in the
/// species' bone order, with the pupils to blink.
pub struct Rig {
    /// The bone names, in the species' order (a joint is named by its index here).
    pub names: Vec<String>,
    pub bones: Vec<Obj>,
    /// Each bone's rest offset (its `at`).
    pub rest: Vec<[f32; 3]>,
    pub pupils: Vec<Obj>,
}

impl Rig {
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }
}

/// Builds a robot's bone tree under `root` and its meshes from the baked species, with the robot's materials.
pub fn instantiate(sp: &Species, geo: &SpeciesGeo, mats: &RobotMats, root: &Obj) -> Rig {
    let names: Vec<String> = sp.bones.keys().cloned().collect();
    let index = |n: &str| names.iter().position(|x| x == n);
    let mut made: Vec<Option<Obj>> = vec![None; names.len()];
    // A bone is made once its parent is (the species' bones form a tree, see validate.rs).
    loop {
        let mut progress = false;
        for (i, name) in names.iter().enumerate() {
            if made[i].is_some() {
                continue;
            }
            let def = &sp.bones[name];
            let parent = if def.parent == "root" {
                Some(root.clone())
            } else {
                index(&def.parent).and_then(|p| made[p].clone())
            };
            if let Some(parent) = parent {
                let g = Obj::group();
                g.set_name(name)
                    .set_position(def.at[0], def.at[1], def.at[2]);
                parent.add(&g);
                made[i] = Some(g);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    // A bone the tree could not place (never, for a valid species) hangs from the root.
    let bones: Vec<Obj> = made
        .into_iter()
        .map(|b| {
            b.unwrap_or_else(|| {
                let g = Obj::group();
                root.add(&g);
                g
            })
        })
        .collect();
    let mut pupils = Vec::new();
    for (name, bg) in &geo.bones {
        if let Some(i) = index(name) {
            add_bone(&bones[i], bg, mats, &mut pupils);
        }
    }
    let rest = sp.bones.values().map(|b| b.at).collect();
    Rig {
        names,
        bones,
        rest,
        pupils,
    }
}
