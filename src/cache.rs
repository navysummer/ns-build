//! 增量构建缓存：基于输入内容的 blake3 指纹，实现精确的"未变即跳过"。
//!
//! 缓存条目（`cache_dir/<hash>.json`）记录任务输入指纹与产物路径，
//! 只有当"全部输入哈希一致"且"产物存在且未过期"时才复用。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil::{atomic_write, hash_file};

/// 单个缓存条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// 任务标识（target:kind:subtask）。
    pub key: String,
    /// 输入指纹（源文件路径 → 内容哈希）。
    pub inputs: BTreeMap<String, String>,
    /// 命令指纹（影响结果的命令部分）。
    pub cmd_hash: String,
    /// 产物路径。
    pub outputs: Vec<String>,
}

/// 构建缓存：内存 map + 磁盘持久化。
#[derive(Debug, Default)]
pub struct Cache {
    dir: PathBuf,
    entries: BTreeMap<String, CacheEntry>,
}

impl Cache {
    /// 加载缓存目录。
    pub fn load(dir: &Path) -> Result<Self, String> {
        let mut cache = Self {
            dir: dir.to_path_buf(),
            entries: BTreeMap::new(),
        };
        if dir.exists() {
            for entry in std::fs::read_dir(dir)
                .map_err(|e| format!("读取缓存目录失败: {e}"))?
            {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        if let Ok(ce) = serde_json::from_str::<CacheEntry>(&text) {
                            cache.entries.insert(ce.key.clone(), ce);
                        }
                    }
                }
            }
        }
        Ok(cache)
    }

    /// 判断任务是否命中：全部输入哈希一致、命令指纹一致、产物均存在。
    pub fn hit(&self, key: &str, inputs: &[PathBuf], cmd_hash: &str) -> bool {
        let Some(entry) = self.entries.get(key) else {
            return false;
        };
        if entry.cmd_hash != cmd_hash {
            return false;
        }
        if entry.inputs.len() != inputs.len() {
            return false;
        }
        for p in inputs {
            let Some(s) = p.to_str() else {
                return false;
            };
            let Some(h) = entry.inputs.get(s) else {
                return false;
            };
            let Ok(cur) = hash_file(p) else {
                return false;
            };
            if &cur != h {
                return false;
            }
        }
        entry.outputs.iter().all(|o| Path::new(o).exists())
    }

    /// 记录任务结果。
    pub fn record(&mut self, key: String, inputs: &[PathBuf], cmd_hash: String, outputs: Vec<String>) {
        let mut imap = BTreeMap::new();
        for p in inputs {
            if let Some(s) = p.to_str() {
                if let Ok(h) = hash_file(p) {
                    imap.insert(s.to_string(), h);
                }
            }
        }
        self.entries.insert(
            key.clone(),
            CacheEntry {
                key,
                inputs: imap,
                cmd_hash,
                outputs,
            },
        );
    }

    /// 持久化到磁盘。
    pub fn save(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("创建缓存目录 {} 失败: {e}", self.dir.display()))?;
        for (key, entry) in &self.entries {
            let name = blake3::hash(key.as_bytes()).to_hex().to_string();
            let path = self.dir.join(format!("{name}.json"));
            let text = serde_json::to_string(entry).map_err(|e| e.to_string())?;
            atomic_write(&path, text.as_bytes())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("nsbuild-cache-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn miss_then_hit() {
        let d = tmpdir();
        let src = d.join("a.c");
        std::fs::write(&src, "int x;\n").unwrap();
        let out = d.join("a.o");
        std::fs::write(&out, "obj").unwrap();

        let mut cache = Cache::load(&d).unwrap();
        assert!(!cache.hit("t:o:0", &[src.clone()], "h1"));

        cache.record("t:o:0".into(), &[src.clone()], "h1".into(), vec![out.to_string_lossy().into()]);
        assert!(cache.hit("t:o:0", &[src.clone()], "h1"));

        // 命令指纹变化 → miss。
        assert!(!cache.hit("t:o:0", &[src.clone()], "h2"));
    }

    #[test]
    fn persists_across_load() {
        let d = tmpdir();
        let src = d.join("a.c");
        std::fs::write(&src, "int x;\n").unwrap();
        let out = d.join("a.o");
        std::fs::write(&out, "obj").unwrap();

        let mut cache = Cache::load(&d).unwrap();
        cache.record("k".into(), &[src.clone()], "cmd".into(), vec![out.to_string_lossy().into()]);
        cache.save().unwrap();

        let cache2 = Cache::load(&d).unwrap();
        assert!(cache2.hit("k", &[src], "cmd"));
    }
}
