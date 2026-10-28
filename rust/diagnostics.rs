//! Ограниченные журналы OCR; включаются явно в настройках. Токены и фиты не пишем.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const LOG_LIMIT: u64 = 4 * 1024 * 1024;
const SNAPSHOT_LIMIT: u64 = 64 * 1024 * 1024;
const SNAPSHOT_COUNT: usize = 80;

pub struct DebugLog {
    directory: PathBuf,
    error: Option<String>,
    sequence: u64,
    stem: &'static str,
}
impl DebugLog {
    pub fn new(directory: PathBuf) -> Self {
        Self::named(directory, "ocr")
    }
    fn named(directory: PathBuf, stem: &'static str) -> Self {
        Self {
            directory,
            error: None,
            sequence: 0,
            stem,
        }
    }
    fn remember<T>(&mut self, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.error = Some(format!("Не удалось записать диагностику: {error}"));
                None
            }
        }
    }
    pub fn event(&mut self, enabled: bool, event: Value) {
        if !enabled {
            return;
        }
        let result = self.write_event(event);
        self.remember(result);
    }
    fn write_event(&self, event: Value) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let path = self.directory.join(format!("{}.jsonl", self.stem));
        if fs::metadata(&path).is_ok_and(|m| m.len() >= LOG_LIMIT) {
            let oldest = self.directory.join(format!("{}.3.jsonl", self.stem));
            if oldest.exists() {
                fs::remove_file(oldest).map_err(|e| e.to_string())?;
            }
            for index in (1..3).rev() {
                let from = self.directory.join(format!("{}.{index}.jsonl", self.stem));
                if from.exists() {
                    fs::rename(
                        from,
                        self.directory
                            .join(format!("{}.{}.jsonl", self.stem, index + 1)),
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            fs::rename(&path, self.directory.join(format!("{}.1.jsonl", self.stem)))
                .map_err(|e| e.to_string())?;
        }
        let record = json!({"time": chrono::Utc::now().to_rfc3339(), "version": env!("CARGO_PKG_VERSION"), "event": event});
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        writeln!(file, "{record}").map_err(|e| e.to_string())
    }
    pub fn snapshot(
        &mut self,
        enabled: bool,
        label: &str,
        image: &image::RgbaImage,
    ) -> Option<String> {
        if !enabled {
            return None;
        }
        self.sequence += 1;
        let name = format!(
            "{}-{}-{}-{label}.png",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
            std::process::id(),
            self.sequence
        );
        let result = (|| {
            fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
            image
                .save(self.directory.join(&name))
                .map_err(|e| e.to_string())?;
            prune_snapshots(&self.directory, SNAPSHOT_COUNT, SNAPSHOT_LIMIT)?;
            Ok(name)
        })();
        self.remember(result)
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}
fn prune_snapshots(directory: &Path, max_count: usize, max_bytes: u64) -> Result<(), String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().ends_with(".png") {
            let meta = entry.metadata().map_err(|e| e.to_string())?;
            files.push((entry.path(), meta.len()));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut bytes: u64 = files.iter().map(|(_, len)| len).sum();
    let mut count = files.len();
    for (path, len) in files {
        if count <= max_count && bytes <= max_bytes {
            break;
        }
        fs::remove_file(path).map_err(|e| e.to_string())?;
        bytes -= len;
        count -= 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_logging_writes_nothing_and_enabled_logging_rotates_and_prunes() {
        let dir = std::env::temp_dir().join(format!("eve-debug-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut log = DebugLog::new(dir.clone());
        let image = image::RgbaImage::new(20, 20);
        log.event(false, json!({"state":"disabled"}));
        assert!(log.snapshot(false, "selected", &image).is_none());
        assert!(!dir.exists());
        log.event(true, json!({"state":"test", "text":"Пилот"}));
        let record: Value =
            serde_json::from_str(fs::read_to_string(dir.join("ocr.jsonl")).unwrap().trim())
                .unwrap();
        assert_eq!(record["event"]["text"], "Пилот");
        fs::OpenOptions::new()
            .write(true)
            .open(dir.join("ocr.jsonl"))
            .unwrap()
            .set_len(LOG_LIMIT)
            .unwrap();
        log.event(true, json!({"state":"rotated"}));
        assert!(dir.join("ocr.1.jsonl").exists());
        for _ in 0..4 {
            assert!(log.snapshot(true, "selected", &image).is_some());
        }
        prune_snapshots(&dir, 2, u64::MAX).unwrap();
        assert_eq!(
            fs::read_dir(&dir)
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|s| s == "png"))
                .count(),
            2
        );
        prune_snapshots(&dir, 2, 0).unwrap();
        assert!(log.take_error().is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}

#[derive(Clone)]
pub struct Trace {
    log: std::sync::Arc<std::sync::Mutex<DebugLog>>,
    enabled: bool,
    context: Value,
}
pub fn trace_id(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "{prefix}-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_micros(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
impl Trace {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            log: std::sync::Arc::new(std::sync::Mutex::new(DebugLog::named(directory, "fits"))),
            enabled: false,
            context: Value::Null,
        }
    }
    pub fn job(&self, enabled: bool, context: Value) -> Self {
        Self {
            log: self.log.clone(),
            enabled,
            context,
        }
    }
    pub fn event(&self, kind: &str, fields: Value) {
        if !self.enabled {
            return;
        }
        let mut event = self.context.as_object().cloned().unwrap_or_default();
        if let Some(fields) = fields.as_object() {
            event.extend(fields.clone());
        }
        event.insert("kind".into(), Value::String(kind.into()));
        let mut log = self.log.lock().unwrap_or_else(|p| p.into_inner());
        log.event(true, Value::Object(event));
    }
    pub fn take_error(&self) -> Option<String> {
        self.log
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take_error()
    }
}
pub fn trace_url(input: &str) -> String {
    let Ok(mut url) = url::Url::parse(input) else {
        return "[invalid URL]".into();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let pairs: Vec<_> = url
        .query_pairs()
        .map(|(key, value)| {
            let safe = ["page", "year", "month", "language", "datasource"].contains(&key.as_ref());
            (
                key.to_string(),
                if safe {
                    value.to_string()
                } else {
                    "[redacted]".into()
                },
            )
        })
        .collect();
    url.set_query(None);
    if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
    }
    url.set_fragment(None);
    url.to_string()
}
