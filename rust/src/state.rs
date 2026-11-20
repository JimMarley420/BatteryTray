//! The last reading with a known level for every device ever seen, kept across restarts in state.json
//! (Core/StateStore.cs), so a device that is off still shows where it was left. Same file, same format.

use crate::reading::DeviceReading;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct StateStore {
    path: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    last: BTreeMap<String, DeviceReading>,
    dirty: bool,
}

impl StateStore {
    pub fn default_path() -> PathBuf {
        crate::win::app_data_folder().join("state.json")
    }

    pub fn load() -> Self {
        Self::load_from(Self::default_path())
    }

    pub fn load_from(path: PathBuf) -> Self {
        let last = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(text.trim_start_matches('\u{FEFF}')).unwrap_or_else(|e| {
                crate::log::write(&format!("state.json unreadable, starting empty: {e}"));
                BTreeMap::new()
            }),
            Err(_) => BTreeMap::new(),
        };
        Self { path, inner: Mutex::new(Inner { last, dirty: false }) }
    }

    pub fn empty() -> Self {
        Self { path: Self::default_path(), inner: Mutex::new(Inner { last: BTreeMap::new(), dirty: false }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn ids(&self) -> Vec<String> {
        self.lock().last.keys().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<DeviceReading> {
        self.lock().last.get(id).cloned()
    }

    pub fn update(&self, r: &DeviceReading) {
        if !r.online || r.level.is_none() {
            return;
        }
        let mut inner = self.lock();
        // Only a changed level is worth a write; the timestamp alone is saved on exit
        let changed = match inner.last.get(&r.id) {
            None => true,
            Some(old) => old.level != r.level || old.charging != r.charging || old.name != r.name,
        };
        if changed {
            inner.dirty = true;
        }
        inner.last.insert(r.id.clone(), r.clone());
    }

    pub fn save_if_dirty(&self) {
        if self.lock().dirty {
            self.save();
        }
    }

    pub fn save(&self) {
        let text = {
            let mut inner = self.lock();
            inner.dirty = false;
            serde_json::to_string_pretty(&inner.last)
        };
        let result = (|| -> std::io::Result<()> {
            let text = text.map_err(std::io::Error::other)?;
            if let Some(dir) = self.path.parent() {
                fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            fs::write(&tmp, text)?;
            fs::rename(&tmp, &self.path)
        })();
        if let Err(e) = result {
            crate::log::write(&format!("cannot save state: {e}"));
        }
    }
}
