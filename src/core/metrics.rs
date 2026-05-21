use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMetrics {
    pub timestamp: i64,
    pub command: String,
    pub duration_ms: f64,
    pub num_commits: usize,
    pub num_actions: usize,
    pub table_version: String,
    pub version: String,
}

pub struct MetricsRecorder {
    start: Instant,
    command: String,
}

impl MetricsRecorder {
    pub fn new(command: &str) -> Self {
        Self {
            start: Instant::now(),
            command: command.to_string(),
        }
    }

    pub fn finish(
        &self,
        num_commits: usize,
        num_actions: usize,
        table_version: u64,
        app_version: &str,
    ) {
        let duration = self.start.elapsed();
        let metrics = CommandMetrics {
            timestamp: chrono::Utc::now().timestamp_millis(),
            command: self.command.clone(),
            duration_ms: duration.as_secs_f64() * 1000.0,
            num_commits,
            num_actions,
            table_version: format!("v{}", table_version),
            version: app_version.to_string(),
        };
        if let Ok(json) = serde_json::to_string(&metrics) {
            if let Some(path) = Self::metrics_file() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    let _ = writeln!(f, "{}", json);
                }
            }
        }
    }

    fn metrics_file() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("DELTALENS_DATA_DIR") {
            return Some(PathBuf::from(dir).join("metrics.jsonl"));
        }
        if let Ok(home) = std::env::var("HOME") {
            return Some(
                PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join("deltalens")
                    .join("metrics.jsonl"),
            );
        }
        None
    }

    pub fn read_all() -> Vec<CommandMetrics> {
        let path = match Self::metrics_file() {
            Some(p) => p,
            None => return vec![],
        };
        if !path.exists() {
            return vec![];
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        content
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    pub fn summary() -> MetricsSummary {
        let all = Self::read_all();
        let total_cmds = all.len();
        let total_ms: f64 = all.iter().map(|m| m.duration_ms).sum();
        let avg_ms = if total_cmds > 0 {
            total_ms / total_cmds as f64
        } else {
            0.0
        };
        let p50 = percentile(&all, 0.50);
        let p95 = percentile(&all, 0.95);
        let p99 = percentile(&all, 0.99);

        let mut cmd_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for m in &all {
            *cmd_counts.entry(m.command.clone()).or_insert(0) += 1;
        }

        MetricsSummary {
            total_commands: total_cmds,
            total_duration_ms: total_ms,
            avg_duration_ms: avg_ms,
            p50_ms: p50,
            p95_ms: p95,
            p99_ms: p99,
            command_counts: cmd_counts,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSummary {
    pub total_commands: usize,
    pub total_duration_ms: f64,
    pub avg_duration_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub command_counts: std::collections::HashMap<String, usize>,
}

fn percentile(data: &[CommandMetrics], pct: f64) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = data.iter().map(|m| m.duration_ms).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((sorted.len() as f64) * pct).ceil() as usize - 1;
    let idx = idx.clamp(0, sorted.len() - 1);
    sorted[idx]
}
