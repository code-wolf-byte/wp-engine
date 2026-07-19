//! Scene-graph transforms: an object's `parent` chain composed into a world
//! transform.
//!
//! Most scenes don't need this — an object's authored `origin`/`angles`/`scale`
//! is already world-space. But 8 of 197 real wallpapers parent objects to a
//! transform node (an object with no image of its own) and drive that node
//! instead of the children, so ignoring `parent` puts every child in the wrong
//! place. Both 2D layers and 3D meshes are affected; the C++ reference has no
//! equivalent (it never composes `parent`).
//!
//! `parent` holds an object **id**, not an index — ids run far past the object
//! count in real content (2077 in a 142-object scene).

use crate::engine::camera3d::{identity, mat4_mul, model_matrix, Mat4};
use crate::engine::scene::SceneObject;
use std::collections::HashMap;

/// An object's own (local) transform, before its parents apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            angles: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

impl Transform {
    pub fn matrix(&self) -> Mat4 {
        model_matrix(self.origin, self.angles, self.scale)
    }
}

/// Per-frame SceneScript sources driving this node's own transform. A parent
/// node is often *only* a script (no image), which is exactly why the chain
/// can't be flattened at load.
#[derive(Clone, Default)]
pub struct NodeScripts {
    pub origin: Option<String>,
    pub angles: Option<String>,
    pub scale: Option<String>,
}

impl NodeScripts {
    pub fn is_empty(&self) -> bool {
        self.origin.is_none() && self.angles.is_none() && self.scale.is_none()
    }
}

/// One entry per `scene.objects` element, index-aligned with it.
pub struct Node {
    /// Index into the node list (already resolved from the `parent` id).
    pub parent: Option<usize>,
    /// Authored transform — the base a script updates from, and the value used
    /// as-is when there's no script.
    pub local: Transform,
    pub scripts: NodeScripts,
}

fn vec3_or(obj: &SceneObject, f: impl Fn(&SceneObject) -> Option<&serde_json::Value>, d: [f32; 3]) -> [f32; 3] {
    f(obj)
        .map(crate::engine::model::json_to_animated)
        .and_then(|v| v.as_vec3())
        .unwrap_or(d)
}

fn script_of(obj: &SceneObject, f: impl Fn(&SceneObject) -> Option<&serde_json::Value>) -> Option<String> {
    f(obj)
        .map(crate::engine::model::json_to_animated)
        .and_then(|v| v.script)
}

/// Build the node list for a scene, resolving each `parent` id to an index.
/// A parent that doesn't resolve is dropped (treated as unparented) rather
/// than failing the scene.
pub fn build(objects: &[SceneObject]) -> Vec<Node> {
    let mut by_id: HashMap<i64, usize> = HashMap::new();
    for (i, o) in objects.iter().enumerate() {
        if let Some(id) = o.id {
            by_id.insert(id, i);
        }
    }
    let by_name: HashMap<&str, usize> = objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| Some((o.name.as_deref()?, i)))
        .collect();

    let mut nodes: Vec<Node> = objects
        .iter()
        .map(|o| {
            let parent = o.parent.as_ref().and_then(|p| {
                if let Some(n) = p.as_i64() {
                    by_id.get(&n).copied()
                } else {
                    by_name.get(p.as_str()?).copied()
                }
            });
            Node {
                parent,
                local: Transform {
                    origin: vec3_or(o, |o| o.origin.as_ref(), [0.0; 3]),
                    angles: vec3_or(o, |o| o.angles.as_ref(), [0.0; 3]),
                    scale: vec3_or(o, |o| o.scale.as_ref(), [1.0; 3]),
                },
                scripts: NodeScripts {
                    origin: script_of(o, |o| o.origin.as_ref()),
                    angles: script_of(o, |o| o.angles.as_ref()),
                    scale: script_of(o, |o| o.scale.as_ref()),
                },
            }
        })
        .collect();

    break_cycles(&mut nodes);
    nodes
}

/// Drop any `parent` that takes part in a cycle (including self-parenting), so
/// resolution can't recurse forever on hand-edited content.
fn break_cycles(nodes: &mut [Node]) {
    for i in 0..nodes.len() {
        let mut slow = Some(i);
        let mut fast = Some(i);
        loop {
            fast = fast.and_then(|f| nodes[f].parent);
            fast = fast.and_then(|f| nodes[f].parent);
            slow = slow.and_then(|s| nodes[s].parent);
            match (slow, fast) {
                (Some(s), Some(f)) if s == f => {
                    nodes[i].parent = None;
                    break;
                }
                (_, None) => break,
                _ => {}
            }
        }
    }
}

/// World matrix per node: the chain of local matrices from the root down.
/// `locals` overrides each node's authored transform (that's how a frame's
/// script results get in); pass `None` to use the authored values.
pub fn world_matrices(nodes: &[Node], locals: Option<&[Transform]>) -> Vec<Mat4> {
    let local_of = |i: usize| match locals {
        Some(l) => l[i],
        None => nodes[i].local,
    };
    let mut out: Vec<Option<Mat4>> = vec![None; nodes.len()];
    // Iterative so a deep chain can't blow the stack; each node resolves its
    // ancestors first, memoized.
    for i in 0..nodes.len() {
        if out[i].is_some() {
            continue;
        }
        let mut chain = Vec::new();
        let mut cur = Some(i);
        while let Some(c) = cur {
            if out[c].is_some() {
                break;
            }
            chain.push(c);
            cur = nodes[c].parent;
        }
        // Root-most first, so each node's parent is already resolved.
        for &c in chain.iter().rev() {
            let m = local_of(c).matrix();
            out[c] = Some(match nodes[c].parent {
                Some(p) => mat4_mul(&out[p].expect("parent resolved first"), &m),
                None => m,
            });
        }
    }
    out.into_iter().map(|m| m.unwrap_or_else(identity)).collect()
}

/// Apply a world matrix to a point.
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objs(json: &str) -> Vec<SceneObject> {
        serde_json::from_str(json).expect("valid objects")
    }

    /// The whole point: `parent` is an id, and ids routinely exceed the object
    /// count. Resolving it as an index silently mis-parents or drops it.
    #[test]
    fn resolves_parent_by_id_not_index() {
        let nodes = build(&objs(
            r#"[{"id": 900, "origin": "10 0 0"},
                {"id": 1,   "origin": "1 0 0", "parent": 900}]"#,
        ));
        assert_eq!(nodes[1].parent, Some(0));
        let w = world_matrices(&nodes, None);
        assert_eq!(transform_point(&w[1], [0.0, 0.0, 0.0]), [11.0, 0.0, 0.0]);
    }

    #[test]
    fn composes_parent_scale_and_translation() {
        let nodes = build(&objs(
            r#"[{"id": 1, "origin": "5 0 0", "scale": "2 2 2"},
                {"id": 2, "origin": "3 0 0", "parent": 1}]"#,
        ));
        let w = world_matrices(&nodes, None);
        // Child sits at parent_origin + parent_scale * child_origin.
        assert_eq!(transform_point(&w[1], [0.0, 0.0, 0.0]), [11.0, 0.0, 0.0]);
    }

    /// An unparented object must come out exactly as authored — this is the
    /// 189-of-197 case, and it must not shift.
    #[test]
    fn unparented_object_is_unchanged() {
        let nodes = build(&objs(r#"[{"id": 1, "origin": "7 -2 3"}]"#));
        let w = world_matrices(&nodes, None);
        assert_eq!(transform_point(&w[0], [0.0, 0.0, 0.0]), [7.0, -2.0, 3.0]);
    }

    /// A parent node is usually script-only; `locals` is how the frame's
    /// evaluated value reaches the chain.
    #[test]
    fn locals_override_authored_values() {
        let nodes = build(&objs(
            r#"[{"id": 1, "origin": "0 0 0"},
                {"id": 2, "origin": "1 0 0", "parent": 1}]"#,
        ));
        let mut locals: Vec<Transform> = nodes.iter().map(|n| n.local).collect();
        locals[0].origin = [0.0, 0.0, 10.0]; // as if a script moved the parent
        let w = world_matrices(&nodes, Some(&locals));
        assert_eq!(transform_point(&w[1], [0.0, 0.0, 0.0]), [1.0, 0.0, 10.0]);
    }

    #[test]
    fn cycles_are_broken_not_hung() {
        let nodes = build(&objs(
            r#"[{"id": 1, "parent": 2, "origin": "1 0 0"},
                {"id": 2, "parent": 1, "origin": "2 0 0"},
                {"id": 3, "parent": 3, "origin": "3 0 0"}]"#,
        ));
        let w = world_matrices(&nodes, None);
        assert_eq!(transform_point(&w[2], [0.0; 3]), [3.0, 0.0, 0.0]);
    }

    #[test]
    fn missing_parent_is_treated_as_root() {
        let nodes = build(&objs(r#"[{"id": 1, "parent": 404, "origin": "4 0 0"}]"#));
        assert_eq!(nodes[0].parent, None);
        let w = world_matrices(&nodes, None);
        assert_eq!(transform_point(&w[0], [0.0; 3]), [4.0, 0.0, 0.0]);
    }
}
