use crate::core::model::action::Action;
use crate::core::model::lineage::{LineageEntry, OperationType};
use crate::core::reader::CommitEntry;

pub struct LineageTracer;

impl LineageTracer {
    /// Reconstruct lineage from a slice of commit entries.
    /// Returns entries in ascending version order.
    pub fn trace(entries: &[CommitEntry]) -> Vec<LineageEntry> {
        entries.iter().filter_map(Self::entry_lineage).collect()
    }

    fn entry_lineage(entry: &CommitEntry) -> Option<LineageEntry> {
        // CommitInfo is the source of lineage — look for it in this commit
        let ci = entry.actions.iter().find_map(|a| {
            if let Action::CommitInfo(ci) = a {
                Some(ci)
            } else {
                None
            }
        })?;

        // Tally files added/removed in this commit
        let mut files_added = 0i64;
        let mut files_removed = 0i64;
        let mut bytes_added = 0i64;
        let mut bytes_removed = 0i64;

        for action in &entry.actions {
            match action {
                Action::Add(a) if a.data_change => {
                    files_added += 1;
                    bytes_added += a.size;
                }
                Action::Remove(r) if r.data_change => {
                    files_removed += 1;
                    bytes_removed += r.size.unwrap_or(0);
                }
                _ => {}
            }
        }

        // Extract row count from operationMetrics if available
        let num_output_rows = ci
            .operation_metrics
            .as_ref()
            .and_then(|m| m.get("numOutputRows"))
            .and_then(|v| v.parse::<i64>().ok());

        // Build writer attribution string: prefer engineInfo, fallback to userInfo
        let writer = Self::build_writer_string(ci);

        // Extract predicate from operationParameters (for DELETE/MERGE/UPDATE)
        let predicate = ci
            .operation_parameters
            .get("predicate")
            .or_else(|| ci.operation_parameters.get("condition"))
            .and_then(|v| v.as_str().map(String::from));

        let operation = ci
            .operation
            .parse::<OperationType>()
            .unwrap_or(OperationType::Other(ci.operation.clone()));

        Some(LineageEntry {
            version: entry.version,
            timestamp: ci.timestamp,
            operation,
            operation_raw: ci.operation.clone(),
            writer,
            num_files_added: files_added,
            num_files_removed: files_removed,
            bytes_added,
            bytes_removed,
            num_output_rows,
            predicate,
            user_name: ci.user_name.clone(),
            cluster_id: ci.cluster_id.clone(),
            engine_info: ci.engine_info.clone(),
        })
    }

    fn build_writer_string(ci: &crate::core::model::action::CommitInfoAction) -> String {
        // Priority: engineInfo > notebookInfo > userName > "unknown"
        if let Some(engine) = &ci.engine_info {
            // Extract a clean name: "Apache-Spark/3.5.0 Delta-Lake/3.1.0" → "spark/..."
            let parts: Vec<&str> = engine.split_whitespace().collect();
            if let Some(first) = parts.first() {
                let name = first.split('/').next().unwrap_or(first).to_lowercase();
                if let Some(user) = &ci.user_name {
                    return format!("{}/{}", name, user);
                }
                return name;
            }
        }

        if let Some(notebook) = &ci.notebook_info {
            return format!(
                "notebook_{}",
                &notebook.notebook_id[..8.min(notebook.notebook_id.len())]
            );
        }

        if let Some(user) = &ci.user_name {
            return user.clone();
        }

        "unknown".to_string()
    }
}

/// Filter lineage entries by optional criteria.
pub struct LineageFilter {
    pub since_timestamp: Option<i64>,
    pub until_timestamp: Option<i64>,
    pub operations: Option<Vec<String>>,
    pub user: Option<String>,
}

impl LineageFilter {
    pub fn apply<'a>(&self, entries: &'a [LineageEntry]) -> Vec<&'a LineageEntry> {
        entries
            .iter()
            .filter(|e| {
                if let Some(since) = self.since_timestamp {
                    if e.timestamp < since {
                        return false;
                    }
                }
                if let Some(until) = self.until_timestamp {
                    if e.timestamp > until {
                        return false;
                    }
                }
                if let Some(ops) = &self.operations {
                    let op_upper = e.operation_raw.to_uppercase();
                    if !ops.iter().any(|o| o.to_uppercase() == op_upper) {
                        return false;
                    }
                }
                if let Some(user) = &self.user {
                    let writer_lower = e.writer.to_lowercase();
                    let user_lower = user.to_lowercase();
                    if !writer_lower.contains(&user_lower)
                        && e.user_name
                            .as_deref()
                            .map(|u| !u.to_lowercase().contains(&user_lower))
                            .unwrap_or(true)
                    {
                        return false;
                    }
                }
                true
            })
            .collect()
    }
}
