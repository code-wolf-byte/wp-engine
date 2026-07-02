#![allow(dead_code)]

use std::collections::BTreeMap;

use super::graph::SceneGraph;

#[derive(Debug, Clone)]
pub struct FrameContext {
    pub frame_index: u64,
    pub time: f32,
    pub delta_time: f32,
    pub resolution: [u32; 2],
    pub camera: CameraFrameState,
    pub objects: Vec<ObjectFrameState>,
    pub effect_uniforms: BTreeMap<String, UniformValue>,
}

impl FrameContext {
    pub fn for_graph(graph: &SceneGraph, time: f32, delta_time: f32, frame_index: u64) -> Self {
        let (width, height) = graph.render_size();
        let objects = graph
            .scene
            .objects
            .iter()
            .enumerate()
            .map(|(index, object)| {
                let origin = object.parsed_origin();
                let size = object.parsed_size();
                ObjectFrameState {
                    object_index: index,
                    object_id: object.id,
                    origin: [origin[0] as f32, origin[1] as f32, origin[2] as f32],
                    size: [size[0] as f32, size[1] as f32, size[2] as f32],
                    transform: translation_matrix(
                        origin[0] as f32,
                        origin[1] as f32,
                        origin[2] as f32,
                    ),
                }
            })
            .collect();

        Self {
            frame_index,
            time,
            delta_time,
            resolution: [width, height],
            camera: CameraFrameState::from_graph(graph),
            objects,
            effect_uniforms: BTreeMap::new(),
        }
    }

    pub fn object_state(&self, object_index: usize) -> Option<&ObjectFrameState> {
        self.objects
            .iter()
            .find(|state| state.object_index == object_index)
    }
}

#[derive(Debug, Clone)]
pub struct CameraFrameState {
    pub view_projection: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub center: [f32; 3],
    pub parallax_amount: f32,
}

impl CameraFrameState {
    fn from_graph(graph: &SceneGraph) -> Self {
        let camera = graph.scene.camera.as_ref();
        Self {
            view_projection: identity_matrix(),
            eye: camera
                .and_then(|camera| camera.eye.as_ref())
                .and_then(parse_value_vec3)
                .unwrap_or([0.0, 0.0, 0.0]),
            center: camera
                .and_then(|camera| camera.center.as_ref())
                .and_then(parse_value_vec3)
                .unwrap_or([0.0, 0.0, 0.0]),
            parallax_amount: camera
                .and_then(|camera| camera.parallax_amount)
                .unwrap_or(0.0) as f32,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObjectFrameState {
    pub object_index: usize,
    pub object_id: Option<i64>,
    pub origin: [f32; 3],
    pub size: [f32; 3],
    pub transform: [[f32; 4]; 4],
}

#[derive(Debug, Clone)]
pub enum UniformValue {
    Float(f32),
    Int(i32),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    Mat4([[f32; 4]; 4]),
}

pub fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn translation_matrix(x: f32, y: f32, z: f32) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [x, y, z, 1.0],
    ]
}

fn parse_value_vec3(value: &serde_json::Value) -> Option<[f32; 3]> {
    match value {
        serde_json::Value::String(value) => {
            let parts: Vec<f32> = value
                .split_whitespace()
                .filter_map(|part| part.parse().ok())
                .collect();
            Some([
                parts.first().copied().unwrap_or(0.0),
                parts.get(1).copied().unwrap_or(0.0),
                parts.get(2).copied().unwrap_or(0.0),
            ])
        }
        serde_json::Value::Array(items) if items.len() >= 3 => Some([
            items[0].as_f64().unwrap_or(0.0) as f32,
            items[1].as_f64().unwrap_or(0.0) as f32,
            items[2].as_f64().unwrap_or(0.0) as f32,
        ]),
        _ => None,
    }
}
