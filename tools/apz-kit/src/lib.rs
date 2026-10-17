//! The machine model kit (`assets/hardware/kit`): the ten machines of the Machines view and the machine view, built
//! here as meshes and written as glTF binaries (`.glb`), with their textures as KTX2. The engine loads what this
//! writes; nothing here runs in the console.
//!
//! - [`mesh`]: the primitives the models are made of (boxes with rounded corners and bevelled edges, cylinders,
//!   tori, rings, planes, spheres) and the transforms that place them.
//! - [`glb`]: a scene of named nodes and named materials, written as one `.glb`.
//! - [`ktx2w`]: RGBA8 images and their mip chains, written as KTX2 with zstd supercompression.
//! - [`tex`]: the kit's procedural textures (the grille).
//! - [`models`]: the ten machines.
//! - [`files`]: the kit's files (the models, the textures and the manifest), as `apz-kit build` writes them.
//!
//! The kit's contract with the engine (names the engine looks for):
//! - Materials by name: `graphite`, `graphite_dark`, `aluminium`, `aluminium_dark`, `polymer`, `bezel`, `bay`,
//!   `grille`, `interior`, `shroud`, `blade`, `glass`, `rubber`, `status`, `led`, `signal`, `screen`, `plate`. The
//!   engine colours them per style; the `.glb` carries a neutral base colour for each.
//! - Nodes by name: `status` (the light strip, coloured by the machine's status), `fan` (a rotor, spun about its local
//!   z axis), `slot:<n>` (a VM slot's light, lit per slot), `screen` (painted with the agent console or dark), `plate`
//!   (painted with the machine's name), `anchor:label` and `anchor:port` (empty nodes: where the name tag sits and
//!   where a cable would end).
//! - Levels of detail: the scene's roots are `lod0` (full), `lod1` (simple) and `lod2` (block).
//! - The scene's extras: `{"kind": …, "size": [w, h, d]}`. Units: 1 u ≈ 10 cm; the base stands on y = 0 and the front
//!   faces +z.

pub mod files;
pub mod glb;
pub mod ktx2w;
pub mod mesh;
pub mod models;
pub mod tex;
