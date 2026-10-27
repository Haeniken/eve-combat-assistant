//! Сохраняемые настройки и доступ к необязательным полям ESI/dogma JSON.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub background: f32,
    pub opacity: f32,
    pub scale: f32,
    pub width: f32,
    pub on_top: bool,
    pub click_through: bool,
    pub own_fit: String,
    pub skill_level: u8,
    pub skills: Option<Value>,
    pub ship: String,
    pub pilot: String,
    pub days: u32,
    pub max_variants: usize,
    pub coverage: f64,
    pub min_percent: f64,
    pub max_attackers: usize,
    pub pilot_days: u32,
    pub ocr: bool,
    pub ocr_interval: f64,
    pub region: Option<[i32; 4]>,
    pub excluded: Vec<String>,
    pub language: String,
    #[serde(alias = "auto_sync")]
    pub auto_pilot_lookup: bool,
    pub pause_outside_eve: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            background: 75.,
            opacity: 100.,
            scale: 100.,
            width: 440.,
            on_top: true,
            click_through: false,
            own_fit: String::new(),
            skill_level: 5,
            skills: None,
            ship: "Rifter".into(),
            pilot: String::new(),
            days: 90,
            max_variants: 5,
            coverage: 85.,
            min_percent: 3.,
            max_attackers: 1,
            pilot_days: 30,
            ocr: false,
            ocr_interval: 1.5,
            region: None,
            excluded: Vec::new(),
            language: "eng".into(),
            auto_pilot_lookup: true,
            pause_outside_eve: true,
        }
    }
}
impl Settings {
    pub fn load(dir: &Path) -> Self {
        let mut s: Self = std::fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        s.background = s.background.clamp(0., 100.);
        s.opacity = s.opacity.clamp(20., 100.);
        s.scale = s.scale.clamp(75., 175.);
        s.width = s.width.clamp(380., 700.);
        s.max_variants = s.max_variants.clamp(1, 12);
        s.max_attackers = s.max_attackers.clamp(1, 5);
        s.skill_level = s.skill_level.min(5);
        s
    }
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("settings.tmp"), bytes).map_err(|e| e.to_string())?;
        // Windows rename cannot replace an existing file: use an atomic write-through helper there.
        crate::platform::replace_file(&dir.join("settings.tmp"), &dir.join("settings.json"))
    }
}
pub fn n(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.)
}
pub fn id(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}
pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
pub fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
