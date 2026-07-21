//! Journal under `{data_dir}/workflows/{run_id}/` only.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::types::{WorkflowRunStatus, WorkflowStats};
use crate::security::redact_secrets;

pub struct WorkflowJournal {
    dir: PathBuf,
    journal_path: PathBuf,
    meta_path: PathBuf,
    phases: Vec<String>,
    logs: Vec<String>,
}

impl WorkflowJournal {
    pub fn create(dir: &Path, run_id: &str, name: Option<&str>) -> Result<Self> {
        fs::create_dir_all(dir)
            .with_context(|| format!("create workflow journal dir {}", dir.display()))?;
        let journal_path = dir.join("journal.jsonl");
        let meta_path = dir.join("meta.json");
        let mut journal = Self {
            dir: dir.to_path_buf(),
            journal_path,
            meta_path,
            phases: Vec::new(),
            logs: Vec::new(),
        };
        journal.append_line(&json!({
            "event": "run_started",
            "run_id": run_id,
            "name": name,
        }))?;
        Ok(journal)
    }

    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn write_meta_start(
        &mut self,
        script: &str,
        args: &Value,
        max_parallel: usize,
        max_agents: usize,
        project_root: &Path,
    ) -> Result<()> {
        let script_hash = hex_sha256(script.as_bytes());
        let args_s = serde_json::to_string(args).unwrap_or_default();
        let args_hash = hex_sha256(args_s.as_bytes());
        let meta = json!({
            "status": "running",
            "script_hash": script_hash,
            "args_hash": args_hash,
            "max_parallel": max_parallel,
            "max_agents": max_agents,
            "project_root": project_root.display().to_string(),
        });
        fs::write(&self.meta_path, serde_json::to_vec_pretty(&meta)?)
            .with_context(|| format!("write {}", self.meta_path.display()))?;
        Ok(())
    }

    pub fn record_phase(&mut self, title: &str) {
        self.phases.push(title.to_string());
        let _ = self.append_line(&json!({"event": "phase", "title": title}));
    }

    pub fn record_log(&mut self, message: &str) {
        let redacted = redact_secrets(message);
        self.logs.push(redacted.clone());
        let _ = self.append_line(&json!({"event": "log", "message": redacted}));
    }

    pub fn take_phases(&self) -> Vec<String> {
        self.phases.clone()
    }

    pub fn finalize(
        &mut self,
        run_id: &str,
        status: WorkflowRunStatus,
        stats: &WorkflowStats,
        error: Option<&str>,
    ) -> Result<()> {
        self.append_line(&json!({
            "event": "run_finished",
            "run_id": run_id,
            "status": status,
            "error": error.map(redact_secrets),
        }))?;
        let meta = json!({
            "status": status,
            "stats": stats,
            "phases": self.phases,
            "error": error.map(redact_secrets),
        });
        fs::write(&self.meta_path, serde_json::to_vec_pretty(&meta)?)?;
        Ok(())
    }

    fn append_line(&mut self, value: &Value) -> Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.journal_path)
            .with_context(|| format!("open {}", self.journal_path.display()))?;
        let line = serde_json::to_string(value)?;
        writeln!(f, "{line}")?;
        Ok(())
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[allow(dead_code)]
fn _touch_file(path: &Path) -> Result<File> {
    File::create(path).with_context(|| format!("create {}", path.display()))
}
