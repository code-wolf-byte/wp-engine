//! project.json user-property resolution.
//!
//! Mirrors linux-wallpaperengine's PropertyParser + UserSettingParser flow:
//! `project.json` declares `general.properties` (sliders, colors, booleans,
//! combos); any value inside `scene.json` may reference one with
//! `{"user": "propname", "value": <default>}`. The effective value is the
//! property's configured value, which the user may override per-run with
//! `--set-property name=value` (the C++ `--set-property` equivalent).

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// One declared property from project.json `general.properties`.
#[derive(Debug, Clone)]
pub struct SceneProperty {
    pub name: String,
    /// Declared type: "slider" | "color" | "bool" | "combo" | "text" | ...
    pub kind: String,
    /// Human-readable label ("ui_browse_properties_..." keys are common).
    pub text: String,
    /// The effective raw value (project default, then CLI override).
    pub value: Value,
}

/// The resolved property set for one wallpaper project.
#[derive(Debug, Clone, Default)]
pub struct SceneProperties {
    properties: HashMap<String, SceneProperty>,
}

impl SceneProperties {
    /// Load `general.properties` from the project.json inside `dir`.
    /// Returns an empty set when there is no project.json or no properties —
    /// scenes without user settings are the common case.
    pub fn from_project_dir(dir: &Path) -> Self {
        let mut props = Self::default();
        let path = dir.join("project.json");
        let Ok(data) = std::fs::read_to_string(&path) else {
            return props;
        };
        let Ok(json) = serde_json::from_str::<Value>(&data) else {
            return props;
        };
        props.load_from_project_json(&json);
        props.apply_overrides(global_overrides().lock().unwrap().clone());
        props
    }

    fn load_from_project_json(&mut self, project: &Value) {
        let Some(map) = project
            .get("general")
            .and_then(|g| g.get("properties"))
            .and_then(|p| p.as_object())
        else {
            return;
        };
        for (name, decl) in map {
            let kind = decl
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let text = decl
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let value = decl.get("value").cloned().unwrap_or(Value::Null);
            self.properties.insert(
                name.clone(),
                SceneProperty {
                    name: name.clone(),
                    kind,
                    text,
                    value,
                },
            );
        }
    }

    /// Apply `name=value` overrides, converting the raw string to the
    /// property's declared type (mirrors the C++ `--set-property` handling).
    pub fn apply_overrides(&mut self, overrides: HashMap<String, String>) {
        for (name, raw) in overrides {
            match self.properties.get_mut(&name) {
                Some(prop) => prop.value = convert_override(&prop.kind, &raw),
                None => {
                    // Unknown property: keep it anyway so scenes that reference
                    // undeclared names (creators do this) still resolve.
                    self.properties.insert(
                        name.clone(),
                        SceneProperty {
                            name,
                            kind: String::new(),
                            text: String::new(),
                            value: guess_value(&raw),
                        },
                    );
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.properties.get(name).map(|p| &p.value)
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
    }

    /// Iterate declared properties (for `list-properties`).
    pub fn iter(&self) -> impl Iterator<Item = &SceneProperty> {
        let mut all: Vec<&SceneProperty> = self.properties.values().collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all.into_iter()
    }

    /// Recursively resolve `{"user": ...}` references in a scene JSON tree.
    ///
    /// Wherever an object carries a `user` key naming a known property, its
    /// `value` is replaced by the property's effective value. The `{"value":X}`
    /// wrapper is preserved because downstream parsers unwrap it. Conditional
    /// references (`"user": {"name":..., "condition":...}`) resolve by name;
    /// the condition expression itself is not evaluated yet.
    pub fn resolve_scene_json(&self, node: &mut Value) {
        if self.properties.is_empty() {
            return;
        }
        self.resolve_node(node);
    }

    fn resolve_node(&self, node: &mut Value) {
        match node {
            Value::Object(map) => {
                let user_name = match map.get("user") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Object(user)) => user
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string()),
                    _ => None,
                };
                if let Some(name) = user_name {
                    if let Some(prop) = self.properties.get(&name) {
                        if !prop.value.is_null() {
                            map.insert("value".into(), prop.value.clone());
                        }
                    }
                }
                for (_, child) in map.iter_mut() {
                    self.resolve_node(child);
                }
            }
            Value::Array(items) => {
                for child in items.iter_mut() {
                    self.resolve_node(child);
                }
            }
            _ => {}
        }
    }
}

fn convert_override(kind: &str, raw: &str) -> Value {
    match kind {
        "bool" => Value::Bool(matches!(raw, "1" | "true" | "yes" | "on")),
        "slider" => raw
            .parse::<f64>()
            .ok()
            .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            .unwrap_or_else(|| Value::String(raw.to_string())),
        "combo" => raw
            .parse::<i64>()
            .map(|i| Value::Number(i.into()))
            .unwrap_or_else(|_| Value::String(raw.to_string())),
        // color properties are "r g b" strings in WE; keep raw text
        _ => Value::String(raw.to_string()),
    }
}

fn guess_value(raw: &str) -> Value {
    if let Ok(i) = raw.parse::<i64>() {
        return Value::Number(i.into());
    }
    if let Ok(f) = raw.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    match raw {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(raw.to_string()),
    }
}

// ── Global override store ─────────────────────────────────────────────────────
// Set once at startup from CLI/application context; consulted every time a
// scene's properties are loaded (matches the C++ ApplicationContext's
// settings.general.properties map).

fn global_overrides() -> &'static Mutex<HashMap<String, String>> {
    static OVERRIDES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    OVERRIDES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Install `--set-property` overrides for all scenes loaded in this process.
pub fn set_global_overrides(overrides: HashMap<String, String>) {
    *global_overrides().lock().unwrap() = overrides;
}

/// Parse a `name=value` CLI argument (bare `name` means boolean true).
pub fn parse_property_arg(arg: &str) -> (String, String) {
    match arg.split_once('=') {
        Some((name, value)) => (name.trim().to_string(), value.to_string()),
        None => (arg.trim().to_string(), "1".to_string()),
    }
}

/// List a project's declared properties (for the `list-properties` command).
pub fn list_properties(dir: &Path) -> Result<Vec<SceneProperty>> {
    let path = dir.join("project.json");
    let data =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let json: Value =
        serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))?;
    let mut props = SceneProperties::default();
    props.load_from_project_json(&json);
    Ok(props.iter().cloned().collect())
}
