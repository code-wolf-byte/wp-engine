#![allow(dead_code)]

use std::collections::BTreeMap;

use super::{camera::SceneCamera, graph::SceneGraph};

#[derive(Debug, Clone)]
pub struct FrameContext {
    pub frame_index: u64,
    pub time: f32,
    pub delta_time: f32,
    pub resolution: [u32; 2],
    pub camera: CameraFrameState,
    pub mouse: MouseFrameState,
    pub audio: AudioFrameState,
    pub objects: Vec<ObjectFrameState>,
    pub effect_uniforms: BTreeMap<String, UniformValue>,
}

impl FrameContext {
    pub fn for_graph(graph: &SceneGraph, time: f32, delta_time: f32, frame_index: u64) -> Self {
        let (width, height) = graph.render_size();
        let camera = CameraFrameState::from_graph(graph);
        let objects = graph
            .scene
            .objects
            .iter()
            .enumerate()
            .map(|(index, object)| {
                let origin = object.parsed_origin();
                let size = object.parsed_size();
                let rotation = object
                    .angles
                    .as_ref()
                    .and_then(parse_value_vec3)
                    .unwrap_or([0.0, 0.0, 0.0]);
                let scale = object
                    .scale
                    .as_ref()
                    .and_then(parse_value_vec3)
                    .unwrap_or([1.0, 1.0, 1.0]);
                let color = object
                    .color
                    .as_ref()
                    .and_then(parse_value_vec3)
                    .unwrap_or([1.0, 1.0, 1.0]);
                ObjectFrameState {
                    object_index: index,
                    object_id: object.id,
                    origin: [origin[0] as f32, origin[1] as f32, origin[2] as f32],
                    size: [size[0] as f32, size[1] as f32, size[2] as f32],
                    scale,
                    color,
                    rotation_z: rotation[2] as f32,
                    alpha: object
                        .alpha
                        .as_ref()
                        .and_then(parse_value_f32)
                        .unwrap_or(1.0),
                    visible: object.is_visible(),
                    parallax_depth: object
                        .parallax_depth
                        .as_ref()
                        .and_then(parse_value_f32)
                        .unwrap_or(0.0),
                    blend_mode: object.color_blend_mode,
                    copy_background: object.copybackground,
                    transform: transform_matrix(
                        origin[0] as f32,
                        origin[1] as f32,
                        origin[2] as f32,
                        scale,
                        rotation[2] as f32,
                    ),
                }
            })
            .collect();
        let mut effect_uniforms = BTreeMap::new();

        insert_uniform(
            &mut effect_uniforms,
            "frame.index",
            UniformValue::Int(frame_index as i32),
        );
        insert_uniform(
            &mut effect_uniforms,
            "frame.time",
            UniformValue::Float(time),
        );
        insert_uniform(
            &mut effect_uniforms,
            "frame.delta_time",
            UniformValue::Float(delta_time),
        );
        insert_uniform(
            &mut effect_uniforms,
            "frame.resolution",
            UniformValue::Vec2([width as f32, height as f32]),
        );
        insert_uniform(
            &mut effect_uniforms,
            "camera.eye",
            UniformValue::Vec3(camera.eye),
        );
        insert_uniform(
            &mut effect_uniforms,
            "camera.center",
            UniformValue::Vec3(camera.center),
        );
        insert_uniform(
            &mut effect_uniforms,
            "camera.parallax_amount",
            UniformValue::Float(camera.parallax_amount),
        );
        insert_uniform(
            &mut effect_uniforms,
            "camera.view_projection",
            UniformValue::Mat4(camera.view_projection),
        );
        insert_uniform(
            &mut effect_uniforms,
            "mouse.position",
            UniformValue::Vec2([0.0, 0.0]),
        );
        insert_uniform(
            &mut effect_uniforms,
            "mouse.buttons",
            UniformValue::Vec2([0.0, 0.0]),
        );
        insert_uniform(
            &mut effect_uniforms,
            "audio.level",
            UniformValue::Float(0.0),
        );
        insert_uniform(
            &mut effect_uniforms,
            "audio.bands",
            UniformValue::Vec4([0.0, 0.0, 0.0, 0.0]),
        );
        collect_effect_uniforms(graph, &mut effect_uniforms);

        Self {
            frame_index,
            time,
            delta_time,
            resolution: [width, height],
            camera,
            mouse: MouseFrameState::default(),
            audio: AudioFrameState::default(),
            objects,
            effect_uniforms,
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
        let scene_camera = SceneCamera::from_scene(&graph.scene, graph.render_size());
        Self {
            view_projection: matrix_from_flat(scene_camera.mvp_matrix()),
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
    pub scale: [f32; 3],
    pub color: [f32; 3],
    pub rotation_z: f32,
    pub alpha: f32,
    pub visible: bool,
    pub parallax_depth: f32,
    pub blend_mode: u32,
    pub copy_background: bool,
    pub transform: [[f32; 4]; 4],
}

#[derive(Debug, Clone, Default)]
pub struct MouseFrameState {
    pub position: [f32; 2],
    pub buttons: [f32; 2],
}

#[derive(Debug, Clone, Default)]
pub struct AudioFrameState {
    pub level: f32,
    pub bands: [f32; 4],
}

#[derive(Debug, Clone, PartialEq)]
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

fn transform_matrix(
    x: f32,
    y: f32,
    z: f32,
    scale: [f32; 3],
    rotation_z_degrees: f32,
) -> [[f32; 4]; 4] {
    let angle = rotation_z_degrees.to_radians();
    let (sin, cos) = angle.sin_cos();
    [
        [scale[0] * cos, scale[0] * sin, 0.0, 0.0],
        [-scale[1] * sin, scale[1] * cos, 0.0, 0.0],
        [0.0, 0.0, scale[2], 0.0],
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
        serde_json::Value::Object(map) => map.get("value").and_then(parse_value_vec3),
        _ => None,
    }
}

fn parse_value_f32(value: &serde_json::Value) -> Option<f32> {
    match value {
        serde_json::Value::Number(value) => value.as_f64().map(|value| value as f32),
        serde_json::Value::String(value) => value.parse().ok(),
        serde_json::Value::Object(map) => map.get("value").and_then(parse_value_f32),
        _ => None,
    }
}

fn matrix_from_flat(values: [f32; 16]) -> [[f32; 4]; 4] {
    [
        [values[0], values[1], values[2], values[3]],
        [values[4], values[5], values[6], values[7]],
        [values[8], values[9], values[10], values[11]],
        [values[12], values[13], values[14], values[15]],
    ]
}

fn collect_effect_uniforms(graph: &SceneGraph, uniforms: &mut BTreeMap<String, UniformValue>) {
    for (object_index, object) in graph.scene.objects.iter().enumerate() {
        let origin = object.parsed_origin();
        let size = object.parsed_size();
        let scale = object
            .scale
            .as_ref()
            .and_then(parse_value_vec3)
            .unwrap_or([1.0, 1.0, 1.0]);
        let color = object
            .color
            .as_ref()
            .and_then(parse_value_vec3)
            .unwrap_or([1.0, 1.0, 1.0]);
        let rotation_z = object
            .angles
            .as_ref()
            .and_then(parse_value_vec3)
            .map(|values| values[2] as f32)
            .unwrap_or(0.0);

        insert_uniform(
            uniforms,
            format!("object[{object_index}].visible"),
            UniformValue::Int(if object.is_visible() { 1 } else { 0 }),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].origin"),
            UniformValue::Vec3([origin[0] as f32, origin[1] as f32, origin[2] as f32]),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].size"),
            UniformValue::Vec3([size[0] as f32, size[1] as f32, size[2] as f32]),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].scale"),
            UniformValue::Vec3([scale[0], scale[1], scale[2]]),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].color"),
            UniformValue::Vec3([color[0], color[1], color[2]]),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].alpha"),
            UniformValue::Float(
                object
                    .alpha
                    .as_ref()
                    .and_then(parse_value_f32)
                    .unwrap_or(1.0),
            ),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].rotation_z"),
            UniformValue::Float(rotation_z),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].parallax_depth"),
            UniformValue::Float(
                object
                    .parallax_depth
                    .as_ref()
                    .and_then(parse_value_f32)
                    .unwrap_or(0.0),
            ),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].blend_mode"),
            UniformValue::Int(object.color_blend_mode as i32),
        );
        insert_uniform(
            uniforms,
            format!("object[{object_index}].copy_background"),
            UniformValue::Int(if object.copybackground { 1 } else { 0 }),
        );

        for (effect_index, effect) in object.effects.iter().enumerate() {
            for (pass_index, pass) in effect.passes.iter().enumerate() {
                for (key, value) in &pass.constantshadervalues {
                    if let Some(uniform) = uniform_value_from_json(value) {
                        insert_uniform(
                            uniforms,
                            format!(
                                "object[{object_index}].effect[{effect_index}].pass[{pass_index}].constant.{key}"
                            ),
                            uniform,
                        );
                    }
                }

                for (key, value) in &pass.combos {
                    insert_uniform(
                        uniforms,
                        format!(
                            "object[{object_index}].effect[{effect_index}].pass[{pass_index}].combo.{key}"
                        ),
                        UniformValue::Int(*value),
                    );
                }

                for (texture_index, texture) in pass.textures.iter().enumerate() {
                    if texture.as_ref().is_some_and(|t| t.file.is_some()) {
                        insert_uniform(
                            uniforms,
                            format!(
                                "object[{object_index}].effect[{effect_index}].pass[{pass_index}].texture[{texture_index}]"
                            ),
                            UniformValue::Int(texture_index as i32),
                        );
                    }
                }
            }
        }
    }
}

fn uniform_value_from_json(value: &serde_json::Value) -> Option<UniformValue> {
    match value {
        serde_json::Value::Number(number) => number
            .as_f64()
            .map(|value| UniformValue::Float(value as f32)),
        serde_json::Value::Bool(value) => Some(UniformValue::Int(if *value { 1 } else { 0 })),
        serde_json::Value::String(value) => {
            let parts: Vec<f32> = value
                .split_whitespace()
                .filter_map(|part| part.parse().ok())
                .collect();
            match parts.len() {
                0 => None,
                1 => Some(UniformValue::Float(parts[0])),
                2 => Some(UniformValue::Vec2([parts[0], parts[1]])),
                3 => Some(UniformValue::Vec3([parts[0], parts[1], parts[2]])),
                4 => Some(UniformValue::Vec4([parts[0], parts[1], parts[2], parts[3]])),
                16 => Some(UniformValue::Mat4([
                    [parts[0], parts[1], parts[2], parts[3]],
                    [parts[4], parts[5], parts[6], parts[7]],
                    [parts[8], parts[9], parts[10], parts[11]],
                    [parts[12], parts[13], parts[14], parts[15]],
                ])),
                _ => None,
            }
        }
        serde_json::Value::Array(items) => {
            let floats: Option<Vec<f32>> = items
                .iter()
                .map(|item| item.as_f64().map(|value| value as f32))
                .collect();
            let floats = floats?;
            match floats.len() {
                1 => Some(UniformValue::Float(floats[0])),
                2 => Some(UniformValue::Vec2([floats[0], floats[1]])),
                3 => Some(UniformValue::Vec3([floats[0], floats[1], floats[2]])),
                4 => Some(UniformValue::Vec4([
                    floats[0], floats[1], floats[2], floats[3],
                ])),
                16 => Some(UniformValue::Mat4([
                    [floats[0], floats[1], floats[2], floats[3]],
                    [floats[4], floats[5], floats[6], floats[7]],
                    [floats[8], floats[9], floats[10], floats[11]],
                    [floats[12], floats[13], floats[14], floats[15]],
                ])),
                _ => None,
            }
        }
        serde_json::Value::Object(map) => map.get("value").and_then(uniform_value_from_json),
        _ => None,
    }
}

fn insert_uniform(
    uniforms: &mut BTreeMap<String, UniformValue>,
    key: impl Into<String>,
    value: UniformValue,
) {
    uniforms.insert(key.into(), value);
}
