//! The debug hooks the hosts read (`window.__apzPerf`, `__apzCamera` and `__apzScene` on the web): the shown view's
//! name, its camera and its scene graph as JSON. Read-only.

use serde_json::{Value, json};

use crate::stage::Stage;

impl Stage {
    /// The shown view's name, as `__apzPerf` gives it: `machine` while the one-machine stage shows, else `machines`.
    pub fn view_name(&self) -> String {
        if self.machine.is_some() { "machine" } else { "machines" }.to_string()
    }

    /// The Machines view's camera as `__apzCamera` reads it: the view, the eye, the target and the field of view in
    /// degrees.
    pub fn camera_json(&mut self) -> String {
        let view = self.view_name();
        let (w, h) = self.size;
        let aspect = if h > 0.0 { w / h } else { 1.0 };
        let cam = self.machines.orbit.camera(aspect);
        json!({
            "view": view,
            "eye": [cam.eye.x, cam.eye.y, cam.eye.z],
            "target": [cam.target.x, cam.target.y, cam.target.z],
            "fov": cam.fov,
        })
        .to_string()
    }
}

/// One node of `scene_json`: three's Object3D fields the old `__apzScene` was read by (type, name, visibility, transform,
/// shadow flags, render order, the userData flags), and its children.
fn node_json(o: &crate::scene::Obj) -> Value {
    use crate::scene::Kind;
    let (head, children) = {
        let n = o.get();
        let (ty, extra) = match &n.kind {
            Kind::Group => ("Group", json!({})),
            Kind::Mesh { geo, .. } => ("Mesh", json!({ "vertices": geo.positions.len() })),
            Kind::Instanced { geo, count, .. } => (
                "Mesh",
                json!({ "vertices": geo.positions.len(), "isInstancedMesh": true, "count": count }),
            ),
            Kind::Points { positions, .. } => ("Points", json!({ "vertices": positions.len() })),
            Kind::PointLight { .. } => ("PointLight", json!({})),
        };
        let mut head = json!({
            "type": ty,
            "name": n.name,
            "visible": n.visible,
            "position": [n.position.x, n.position.y, n.position.z],
            "rotation": [n.rotation.x, n.rotation.y, n.rotation.z],
            "scale": [n.scale.x, n.scale.y, n.scale.z],
            "castShadow": n.cast_shadow,
            "receiveShadow": n.receive_shadow,
            "renderOrder": n.render_order,
            "userData": { "dynamic": n.dynamic, "rest": n.rest, "outline": !n.no_outline },
        });
        if let (Some(h), Some(e)) = (head.as_object_mut(), extra.as_object()) {
            h.extend(e.clone());
        }
        (head, n.children.clone())
    };
    let mut head = head;
    head["children"] = Value::Array(children.iter().map(node_json).collect());
    head
}

impl Stage {
    /// The shown view's scene graph as JSON (`__apzScene`, three's Scene object in the retired build): `{ type: "Scene",
    /// children }`, each node with three's Object3D fields.
    pub fn scene_json(&mut self) -> String {
        let root = self.machines.root.clone();
        let children: Vec<Value> = root.get().children.clone().iter().map(node_json).collect();
        json!({ "type": "Scene", "name": "", "children": children }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::stage::Stage;

    #[test]
    fn the_hooks_read_the_machines_view() {
        let mut stage = Stage::default();
        assert_eq!(stage.view_name(), "machines");
        let cam: Value = serde_json::from_str(&stage.camera_json()).unwrap();
        assert_eq!(cam["view"], "machines");
        assert!(cam["fov"].is_number());
        let scene: Value = serde_json::from_str(&stage.scene_json()).unwrap();
        assert_eq!(scene["type"], "Scene");
        assert!(!scene["children"].as_array().unwrap().is_empty(), "the sky and the ground are in the scene");
    }
}
