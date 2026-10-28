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
}
impl DebugLog {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            error: None,
            sequence: 0,
        }
    }
    fn remember<T>(&mut self, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.error = Some(format!("Не удалось записать диагностику OCR: {error}"));
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
        let path = self.directory.join("ocr.jsonl");
        if fs::metadata(&path).is_ok_and(|m| m.len() >= LOG_LIMIT) {
            let oldest = self.directory.join("ocr.3.jsonl");
            if oldest.exists() {
                fs::remove_file(oldest).map_err(|e| e.to_string())?;
            }
            for index in (1..3).rev() {
                let from = self.directory.join(format!("ocr.{index}.jsonl"));
                if from.exists() {
                    fs::rename(
                        from,
                        self.directory.join(format!("ocr.{}.jsonl", index + 1)),
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            fs::rename(&path, self.directory.join("ocr.1.jsonl")).map_err(|e| e.to_string())?;
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
