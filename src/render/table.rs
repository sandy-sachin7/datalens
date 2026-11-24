use crate::core::analyzer::diff::VersionDiff;
use crate::core::model::lineage::{LineageEntry, OperationType};
use crate::core::model::schema::{SchemaChange, SchemaSnapshot};
use crate::core::model::stats::TableStats;
use bytesize::ByteSize;
use chrono::{TimeZone, Utc};
use colored::Colorize;
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, Color, ContentArrangement, Table};
use std::cmp::Ordering;
use std::io::{self, IsTerminal, Write};

const NONE_DISPLAY: &str = "(none)";

/// Centralized, panic-free UTC timestamp formatter.
pub fn format_timestamp_ms(ts: i64, fmt: &str) -> String {
    match Utc.timestamp_millis_opt(ts).single() {
        Some(dt) => dt.format(fmt).to_string(),
        None => format!("invalid timestamp ({ts})"),
    }
}

/// Panic-free non-negative byte formatter.
pub fn format_bytes(bytes: i64) -> String {
    match u64::try_from(bytes) {
        Ok(b) => ByteSize::b(b).to_string(),
        Err(_) => format!("-{}", ByteSize::b(bytes.unsigned_abs())),
    }
}

/// Panic-free signed byte delta formatter (safe even for `i64::MIN`).
pub fn format_signed_bytes(delta: i64) -> String {
    if delta >= 0 {
        format!("+{}", ByteSize::b(delta.unsigned_abs()))
    } else {
        format!("-{}", ByteSize::b(delta.unsigned_abs()))
    }
}

fn format_list(items: &[String]) -> String {
    if items.is_empty() {
        NONE_DISPLAY.to_string()
    } else {
        items.join(", ")
    }
}

fn format_rows(rows: i64) -> String {
    let abs_r = rows.unsigned_abs();
    let formatted_r = if abs_r >= 1_000_000 {
        format!("{:.1}M", rows as f64 / 1_000_000.0)
    } else if abs_r >= 1_000 {
        format!("{:.1}K", rows as f64 / 1_000.0)
    } else {
        rows.to_string()
    };
    match rows.cmp(&0) {
        Ordering::Greater => format!("+{formatted_r} rows"),
        Ordering::Less => format!("{formatted_r} rows"),
        Ordering::Equal => "±0 rows".to_string(),
    }
}

pub struct TableRenderer {
    plain: bool,
    no_header: bool,
    use_color: bool,
}

/// Strict `no-color.org` check: only disable color when `NO_COLOR` is set AND non-empty.
fn should_use_color(
    plain: bool,
    no_color_env: Option<&std::ffi::OsStr>,
    is_terminal: bool,
) -> bool {
    let no_color = no_color_env.is_some_and(|v| !v.is_empty());
    !plain && !no_color && is_terminal
}

impl TableRenderer {
    pub fn new(plain: bool, no_header: bool) -> Self {
        let use_color = should_use_color(
            plain,
            std::env::var_os("NO_COLOR").as_deref(),
            io::stdout().is_terminal(),
        );
        Self {
            plain,
            no_header,
            use_color,
        }
    }

    /// Explicit constructor for deterministic unit testing.
    pub fn with_options(plain: bool, no_header: bool, use_color: bool) -> Self {
        Self {
            plain,
            no_header,
            use_color: !plain && use_color,
        }
    }

    fn make_table(&self) -> Table {
        let mut table = Table::new();
        table.set_content_arrangement(ContentArrangement::Dynamic);
        if !self.plain {
            table
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS);
        }
        table
    }

    /// Create a cell with color ONLY when `self.use_color` is true (zero ANSI escapes in plain mode).
    fn cell(&self, text: impl Into<String>, color: Option<Color>) -> Cell {
        let cell = Cell::new(text.into());
        if self.use_color {
            if let Some(c) = color {
                return cell.fg(c);
            }
        }
        cell
    }

    fn paint_cyan_bold(&self, text: &str) -> String {
        if self.use_color {
            text.cyan().bold().to_string()
        } else {
            text.to_string()
        }
    }

    fn paint_cyan(&self, text: &str) -> String {
        if self.use_color {
            text.cyan().to_string()
        } else {
            text.to_string()
        }
    }

    fn paint_gray(&self, text: &str) -> String {
        if self.use_color {
            text.truecolor(128, 128, 128).to_string()
        } else {
            text.to_string()
        }
    }

    fn paint_green(&self, text: &str) -> String {
        if self.use_color {
            text.green().to_string()
        } else {
            text.to_string()
        }
    }

    fn paint_red(&self, text: &str) -> String {
        if self.use_color {
            text.red().to_string()
        } else {
            text.to_string()
        }
    }

    fn paint_yellow(&self, text: &str) -> String {
        if self.use_color {
            text.yellow().to_string()
        } else {
            text.to_string()
        }
    }

    fn paint_op(&self, op: &OperationType, padded: &str) -> String {
        if !self.use_color {
            return padded.to_string();
        }
        match op {
            OperationType::Write => padded.green().to_string(),
            OperationType::Merge => padded.blue().to_string(),
            OperationType::Delete => padded.red().to_string(),
            OperationType::Update => padded.yellow().to_string(),
            OperationType::Optimize => padded.magenta().to_string(),
            _ => padded.to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // inspect
    // -----------------------------------------------------------------------

    pub fn render_inspect(&self, path: &str, stats: &TableStats) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        self.render_inspect_to(&mut stdout, path, stats)
    }

    pub fn render_inspect_to<W: Write>(
        &self,
        w: &mut W,
        path: &str,
        stats: &TableStats,
    ) -> io::Result<()> {
        let mut table = self.make_table();

        if !self.no_header {
            table.set_header(vec![
                self.cell("deltalens · Table Health Report", Some(Color::Cyan)),
                self.cell(path, Some(Color::Cyan)),
            ]);
        }

        table.add_row(vec![
            "Current Version".to_string(),
            format!("v{:03}", stats.current_version),
        ]);

        if let Some(created) = stats.created_timestamp {
            table.add_row(vec![
                "Created".to_string(),
                format_timestamp_ms(created, "%Y-%m-%d %H:%M:%S UTC"),
            ]);
        }
        if let Some(modified) = stats.last_modified_timestamp {
            table.add_row(vec![
                "Last Modified".to_string(),
                format_timestamp_ms(modified, "%Y-%m-%d %H:%M:%S UTC"),
            ]);
        }

        let mut proto = format!(
            "reader={}, writer={}",
            stats.min_reader_version, stats.min_writer_version
        );
        if !stats.reader_features.is_empty() || !stats.writer_features.is_empty() {
            proto.push_str(&format!(
                " (r:[{}], w:[{}])",
                format_list(&stats.reader_features),
                format_list(&stats.writer_features)
            ));
        }
        table.add_row(vec!["Protocol".to_string(), proto]);

        table.add_row(vec![
            self.cell("DATA FILES", Some(Color::Yellow)),
            self.cell("", None),
        ]);
        table.add_row(vec![
            "Total Files".to_string(),
            stats.health.total_files.to_string(),
        ]);
        table.add_row(vec![
            "Total Size".to_string(),
            format_bytes(stats.health.total_size),
        ]);
        table.add_row(vec![
            "Avg File Size".to_string(),
            format_bytes(stats.health.avg_file_size),
        ]);

        let skew_warn = stats.health.skew_score > 0.5;
        let warn_color = if skew_warn { Some(Color::Red) } else { None };
        table.add_row(vec![
            self.cell("Min File Size", None),
            self.cell(format_bytes(stats.health.min_file_size), warn_color),
        ]);
        table.add_row(vec![
            self.cell("Max File Size", None),
            self.cell(format_bytes(stats.health.max_file_size), warn_color),
        ]);

        let small_warn = stats.health.total_files > 1 && stats.health.small_file_pct > 20.0;
        let small_val = format!(
            "{} ({:.1}%)",
            stats.health.small_file_count, stats.health.small_file_pct
        );
        table.add_row(vec![
            self.cell("Small Files (<32MB)", None),
            self.cell(small_val, if small_warn { Some(Color::Red) } else { None }),
        ]);

        table.add_row(vec![
            self.cell("Skew Score", None),
            self.cell(format!("{:.2} / 1.0", stats.health.skew_score), warn_color),
        ]);

        if stats.health.deletion_vector_files > 0 {
            table.add_row(vec![
                "Deletion Vectors".to_string(),
                format!(
                    "{} files ({} deleted rows)",
                    stats.health.deletion_vector_files, stats.health.deleted_rows_in_dvs
                ),
            ]);
        }

        table.add_row(vec![
            self.cell("PARTITIONS", Some(Color::Yellow)),
            self.cell("", None),
        ]);
        table.add_row(vec![
            "Partition Columns".to_string(),
            format_list(&stats.partition_columns),
        ]);
        table.add_row(vec![
            "Partition Count".to_string(),
            stats.partition_count.to_string(),
        ]);
        table.add_row(vec![
            "Empty Partitions".to_string(),
            stats.empty_partition_count.to_string(),
        ]);

        table.add_row(vec![
            self.cell("MAINTENANCE", Some(Color::Yellow)),
            self.cell("", None),
        ]);

        let last_vac = match stats.maintenance.last_vacuum_version {
            Some(v) => match stats.maintenance.last_vacuum_timestamp {
                Some(ts) if ts > 0 => format!(
                    "v{:03} ({})",
                    v,
                    format_timestamp_ms(ts, "%Y-%m-%d %H:%M:%S UTC")
                ),
                _ => format!("v{:03}", v),
            },
            None => match stats.maintenance.last_checkpoint_version {
                Some(ckpt_v) => format!("Not observed since checkpoint v{:03}", ckpt_v),
                None => "Never".to_string(),
            },
        };
        table.add_row(vec!["Last VACUUM".to_string(), last_vac]);

        let last_opt = match stats.maintenance.last_optimize_version {
            Some(v) => match stats.maintenance.last_optimize_timestamp {
                Some(ts) if ts > 0 => format!(
                    "v{:03} ({})",
                    v,
                    format_timestamp_ms(ts, "%Y-%m-%d %H:%M:%S UTC")
                ),
                _ => format!("v{:03}", v),
            },
            None => match stats.maintenance.last_checkpoint_version {
                Some(ckpt_v) => format!("Not observed since checkpoint v{:03}", ckpt_v),
                None => "Never".to_string(),
            },
        };
        table.add_row(vec!["Last OPTIMIZE".to_string(), last_opt]);

        let last_ckpt = match stats.maintenance.last_checkpoint_version {
            Some(v) => format!("v{:03}", v),
            None => NONE_DISPLAY.to_string(),
        };
        table.add_row(vec!["Last CHECKPOINT".to_string(), last_ckpt]);

        table.add_row(vec![
            "Z-Order Columns".to_string(),
            format_list(&stats.maintenance.z_order_columns),
        ]);
        if !stats.maintenance.clustering_columns.is_empty() {
            table.add_row(vec![
                "Clustering Columns".to_string(),
                format_list(&stats.maintenance.clustering_columns),
            ]);
        }

        let tomb_val = if stats.tombstones.active_tombstones == 0 {
            "0".to_string()
        } else {
            let mut s = format!(
                "{} ({} reclaimable)",
                stats.tombstones.active_tombstones,
                format_bytes(stats.tombstones.reclaimable_bytes)
            );
            if let Some(oldest) = stats.tombstones.oldest_tombstone_timestamp {
                s.push_str(&format!(
                    ", oldest {}",
                    format_timestamp_ms(oldest, "%Y-%m-%d")
                ));
            }
            s
        };
        table.add_row(vec!["Active Tombstones".to_string(), tomb_val]);

        table.add_row(vec![
            self.cell("SCHEMA", Some(Color::Yellow)),
            self.cell("", None),
        ]);
        table.add_row(vec![
            "Column Count".to_string(),
            stats.schema_column_count.to_string(),
        ]);
        table.add_row(vec![
            "Schema Changes".to_string(),
            format!("{} (in analyzed log)", stats.schema_change_count),
        ]);

        writeln!(w, "{table}")?;

        let issues = stats.health_issues();
        if !issues.is_empty() {
            let prefix = if self.plain { "WARN: " } else { "⚠ " };
            let noun = if issues.len() == 1 { "issue" } else { "issues" };
            let summary = format!("{prefix}{} {noun} detected:", issues.len());
            writeln!(w, "\n{}", self.paint_yellow(&summary))?;
            for issue in &issues {
                writeln!(w, "  - {issue}")?;
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // diff
    // -----------------------------------------------------------------------

    pub fn render_diff(
        &self,
        path: &str,
        diff: &VersionDiff,
        schema_only: bool,
        files_only: bool,
    ) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        self.render_diff_to(&mut stdout, path, diff, schema_only, files_only)
    }

    pub fn render_diff_to<W: Write>(
        &self,
        w: &mut W,
        path: &str,
        diff: &VersionDiff,
        schema_only: bool,
        files_only: bool,
    ) -> io::Result<()> {
        let mut table = self.make_table();

        if !self.no_header {
            table.set_header(vec![
                self.cell(
                    format!(
                        "deltalens · Version Diff  v{:03} → v{:03}",
                        diff.v1, diff.v2
                    ),
                    Some(Color::Cyan),
                ),
                self.cell(path, Some(Color::Cyan)),
            ]);
        }

        if !files_only {
            table.add_row(vec![
                self.cell("SCHEMA CHANGES", Some(Color::Yellow)),
                self.cell("", None),
            ]);

            if diff.schema_changes.is_empty() {
                table.add_row(vec!["No schema changes detected", ""]);
            } else {
                for change in &diff.schema_changes {
                    for col in &change.added_columns {
                        table.add_row(vec![
                            self.cell("+ added", Some(Color::Green)),
                            self.cell(col, None),
                        ]);
                    }
                    for col in &change.removed_columns {
                        table.add_row(vec![
                            self.cell("- removed [BREAKING]", Some(Color::Red)),
                            self.cell(col, None),
                        ]);
                    }
                    for ren in &change.renamed_columns {
                        table.add_row(vec![
                            self.cell("→ renamed [BREAKING]", Some(Color::Yellow)),
                            self.cell(format!("{} → {}", ren.old_name, ren.new_name), None),
                        ]);
                    }
                    for m in &change.modified_columns {
                        let tag = if m.breaking {
                            "~ modified [BREAKING]"
                        } else {
                            "~ widened"
                        };
                        let color = if m.breaking {
                            Some(Color::Red)
                        } else {
                            Some(Color::Yellow)
                        };
                        table.add_row(vec![
                            self.cell(tag, color),
                            self.cell(
                                format!("{} ({} → {})", m.name, m.old_type, m.new_type),
                                None,
                            ),
                        ]);
                    }
                    for n in &change.nullability_changes {
                        let tag = if n.breaking {
                            "~ nullable [BREAKING]"
                        } else {
                            "~ nullable"
                        };
                        let color = if n.breaking {
                            Some(Color::Red)
                        } else {
                            Some(Color::Yellow)
                        };
                        table.add_row(vec![
                            self.cell(tag, color),
                            self.cell(
                                format!("{} ({} → {})", n.name, n.old_nullable, n.new_nullable),
                                None,
                            ),
                        ]);
                    }
                }
            }
        }

        if !schema_only {
            table.add_row(vec![
                self.cell("DATA CHANGES", Some(Color::Yellow)),
                self.cell("", None),
            ]);

            table.add_row(vec![
                "Files Added".to_string(),
                format!("+{}", diff.files_added),
            ]);
            table.add_row(vec![
                "Files Removed".to_string(),
                format!("-{}", diff.files_removed),
            ]);
            table.add_row(vec![
                "Net File Delta".to_string(),
                format!("{:+}", diff.files_added.saturating_sub(diff.files_removed)),
            ]);
            table.add_row(vec![
                "Size Added".to_string(),
                format_bytes(diff.bytes_added),
            ]);
            table.add_row(vec![
                "Size Removed".to_string(),
                format_bytes(diff.bytes_removed),
            ]);
            table.add_row(vec![
                "Net Size Delta".to_string(),
                format_signed_bytes(diff.bytes_added.saturating_sub(diff.bytes_removed)),
            ]);

            if diff.physical_files_added != diff.files_added
                || diff.physical_files_removed != diff.files_removed
            {
                table.add_row(vec![
                    "Physical Files (incl. OPTIMIZE)".to_string(),
                    format!(
                        "+{} / -{} ({} net)",
                        diff.physical_files_added,
                        diff.physical_files_removed,
                        format_signed_bytes(
                            diff.physical_bytes_added
                                .saturating_sub(diff.physical_bytes_removed)
                        )
                    ),
                ]);
            }

            table.add_row(vec![
                self.cell(
                    format!("OPERATIONS ({} commits)", diff.total_commits),
                    Some(Color::Yellow),
                ),
                self.cell("", None),
            ]);

            if diff.operations.is_empty() {
                table.add_row(vec!["No operations recorded", ""]);
            } else {
                let mut ops: Vec<_> = diff.operations.iter().collect();
                ops.sort_by_key(|(op, _)| *op);
                for (op, count) in ops {
                    table.add_row(vec![op.as_str(), &count.to_string()]);
                }
            }

            table.add_row(vec![
                self.cell("PARTITION CHANGES", Some(Color::Yellow)),
                self.cell("", None),
            ]);

            let new_part_str = if diff.new_partitions.is_empty() {
                "0".to_string()
            } else {
                format!(
                    "{} ({})",
                    diff.new_partitions.len(),
                    diff.new_partitions.join(", ")
                )
            };
            let rem_part_str = if diff.removed_partitions.is_empty() {
                "0".to_string()
            } else {
                format!(
                    "{} ({})",
                    diff.removed_partitions.len(),
                    diff.removed_partitions.join(", ")
                )
            };
            table.add_row(vec!["New Partitions".to_string(), new_part_str]);
            table.add_row(vec!["Removed Partitions".to_string(), rem_part_str]);
        }

        writeln!(w, "{table}")
    }

    // -----------------------------------------------------------------------
    // lineage
    // -----------------------------------------------------------------------

    pub fn render_lineage(&self, path: &str, entries: &[LineageEntry]) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        self.render_lineage_to(&mut stdout, path, entries)
    }

    pub fn render_lineage_to<W: Write>(
        &self,
        w: &mut W,
        path: &str,
        entries: &[LineageEntry],
    ) -> io::Result<()> {
        if entries.is_empty() {
            writeln!(w, "No lineage entries found.")?;
            return Ok(());
        }

        if !self.no_header {
            writeln!(w, "{}", self.paint_cyan_bold(&format!("Lineage · {path}")))?;
        }

        for entry in entries {
            let ver_pad = format!("{:<6}", format!("v{:03}", entry.version));
            let dt_raw = match entry.timestamp {
                Some(ts) => format_timestamp_ms(ts, "%Y-%m-%d %H:%M"),
                None => "N/A".to_string(),
            };
            let dt_pad = format!("{:<16}", dt_raw);
            let op_pad = format!("{:<14}", entry.operation_raw);
            let writer_pad = format!("{:<24}", entry.writer);

            // Show physical bytes when logical delta is 0 but physical rewrite occurred (e.g. OPTIMIZE).
            let logical_net = entry.bytes_added.saturating_sub(entry.bytes_removed);
            let phys_net = entry
                .physical_bytes_added
                .saturating_sub(entry.physical_bytes_removed);
            let (display_net, is_phys_only) = if entry.num_files_added == 0
                && entry.num_files_removed == 0
                && (entry.physical_files_added > 0 || entry.physical_files_removed > 0)
            {
                (phys_net, true)
            } else {
                (logical_net, false)
            };

            let size_raw = if is_phys_only {
                format!("{} (phys)", format_signed_bytes(display_net))
            } else {
                format_signed_bytes(display_net)
            };
            let size_pad = format!("{:<16}", size_raw);

            let rows_raw = match entry.num_output_rows {
                Some(r) => format_rows(r),
                None => String::new(),
            };

            let ver_out = self.paint_cyan(&ver_pad);
            let dt_out = self.paint_gray(&dt_pad);
            let op_out = self.paint_op(&entry.operation, &op_pad);
            let size_out = if display_net >= 0 {
                self.paint_green(&size_pad)
            } else {
                self.paint_red(&size_pad)
            };
            let rows_out = if entry.num_output_rows.unwrap_or(0) >= 0 {
                self.paint_green(&rows_raw)
            } else {
                self.paint_red(&rows_raw)
            };

            writeln!(
                w,
                "{ver_out}  {dt_out}  {op_out}  {writer_pad}  {size_out}  {rows_out}"
            )?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // audit
    // -----------------------------------------------------------------------

    pub fn render_audit(&self, path: &str, entries: &[LineageEntry]) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        self.render_audit_to(&mut stdout, path, entries)
    }

    pub fn render_audit_to<W: Write>(
        &self,
        w: &mut W,
        path: &str,
        entries: &[LineageEntry],
    ) -> io::Result<()> {
        if entries.is_empty() {
            writeln!(w, "No audit operations found matching the criteria.")?;
            return Ok(());
        }

        if !self.no_header {
            writeln!(
                w,
                "{}",
                self.paint_cyan_bold(&format!("Audit Trail · {path}"))
            )?;
        }

        for entry in entries {
            let ver_pad = format!("{:<6}", format!("v{:03}", entry.version));
            let dt_raw = match entry.timestamp {
                Some(ts) => format_timestamp_ms(ts, "%Y-%m-%d %H:%M"),
                None => "N/A".to_string(),
            };
            let dt_pad = format!("{:<16}", dt_raw);
            let op_pad = format!("{:<14}", entry.operation_raw);
            let writer_pad = format!("{:<20}", entry.writer);

            let predicate_raw = match &entry.predicate {
                Some(p) => format!("predicate=\"{p}\""),
                None => String::new(),
            };
            let pred_pad = format!("{:<35}", predicate_raw);

            let net_bytes = entry.bytes_added.saturating_sub(entry.bytes_removed);
            let (impact_raw, impact_sign) = match entry.num_output_rows {
                Some(r) => (format_rows(r), r),
                None => (format_signed_bytes(net_bytes), net_bytes),
            };

            let ver_out = self.paint_cyan(&ver_pad);
            let dt_out = self.paint_gray(&dt_pad);
            let op_out = self.paint_op(&entry.operation, &op_pad);
            let pred_out = if self.use_color {
                pred_pad.truecolor(140, 180, 250).to_string()
            } else {
                pred_pad
            };
            let impact_out = if impact_sign >= 0 {
                self.paint_green(&impact_raw)
            } else {
                self.paint_red(&impact_raw)
            };

            writeln!(
                w,
                "{ver_out}  {dt_out}  {op_out}  {writer_pad}  {pred_out}  {impact_out}"
            )?;
        }
        writeln!(w, "\n{} operations found.", entries.len())
    }

    // -----------------------------------------------------------------------
    // schema snapshot & history
    // -----------------------------------------------------------------------

    pub fn render_schema_snapshot(&self, path: &str, snapshot: &SchemaSnapshot) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        self.render_schema_snapshot_to(&mut stdout, path, snapshot)
    }

    pub fn render_schema_snapshot_to<W: Write>(
        &self,
        w: &mut W,
        path: &str,
        snapshot: &SchemaSnapshot,
    ) -> io::Result<()> {
        if !self.no_header {
            let title = format!("Schema at v{:03} · {path}", snapshot.version);
            writeln!(w, "{}", self.paint_cyan_bold(&title))?;
        }

        let mut table = self.make_table();

        // Proper 4-column header aligned with the 4 data columns!
        if !self.no_header {
            table.set_header(vec![
                self.cell("Field Name", Some(Color::Yellow)),
                self.cell("Type", Some(Color::Yellow)),
                self.cell("Nullable", Some(Color::Yellow)),
                self.cell("Metadata", Some(Color::Yellow)),
            ]);
        }

        for field in &snapshot.schema.fields {
            let nullable_str = if field.nullable { "true" } else { "false" };
            let type_str = field.data_type.display_name();
            let meta_str = if field.metadata.is_null()
                || field
                    .metadata
                    .as_object()
                    .map(|m| m.is_empty())
                    .unwrap_or(false)
            {
                "-".to_string()
            } else {
                field.metadata.to_string()
            };

            table.add_row(vec![
                field.name.as_str(),
                type_str.as_str(),
                nullable_str,
                meta_str.as_str(),
            ]);
        }

        writeln!(w, "{table}")
    }

    pub fn render_schema_history(&self, path: &str, history: &[SchemaChange]) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        self.render_schema_history_to(&mut stdout, path, history)
    }

    pub fn render_schema_history_to<W: Write>(
        &self,
        w: &mut W,
        path: &str,
        history: &[SchemaChange],
    ) -> io::Result<()> {
        if history.is_empty() {
            writeln!(w, "No schema evolution history detected.")?;
            return Ok(());
        }

        if !self.no_header {
            writeln!(
                w,
                "{}\n",
                self.paint_cyan_bold(&format!("Schema Evolution · {path}"))
            )?;
        }

        for change in history {
            let dt_str = match change.timestamp {
                Some(ts) => format_timestamp_ms(ts, "%Y-%m-%d %H:%M"),
                None => "N/A".to_string(),
            };
            let ver_str = format!("v{:03}", change.version);
            let header_label = if change.has_breaking_changes {
                "Schema Change [BREAKING]"
            } else {
                "Schema Change"
            };

            writeln!(
                w,
                "{}  {}  {}",
                self.paint_cyan(&ver_str),
                self.paint_gray(&dt_str),
                if change.has_breaking_changes {
                    self.paint_red(header_label)
                } else {
                    header_label.to_string()
                }
            )?;

            for col in &change.added_columns {
                let line = format!("+ added: {col}");
                writeln!(w, "      {}", self.paint_green(&line))?;
            }
            for col in &change.removed_columns {
                let line = format!("- removed [BREAKING]: {col}");
                writeln!(w, "      {}", self.paint_red(&line))?;
            }
            for ren in &change.renamed_columns {
                let line = format!("→ renamed [BREAKING]: {} → {}", ren.old_name, ren.new_name);
                writeln!(w, "      {}", self.paint_yellow(&line))?;
            }
            for m in &change.modified_columns {
                let tag = if m.breaking {
                    "~ modified [BREAKING]"
                } else {
                    "~ widened"
                };
                let line = format!("{tag}: {} ({} → {})", m.name, m.old_type, m.new_type);
                let painted = if m.breaking {
                    self.paint_red(&line)
                } else {
                    self.paint_yellow(&line)
                };
                writeln!(w, "      {painted}")?;
            }
            for n in &change.nullability_changes {
                let tag = if n.breaking {
                    "~ nullable [BREAKING]"
                } else {
                    "~ nullable"
                };
                let line = format!(
                    "{tag}: {} ({} → {})",
                    n.name, n.old_nullable, n.new_nullable
                );
                let painted = if n.breaking {
                    self.paint_red(&line)
                } else {
                    self.paint_yellow(&line)
                };
                writeln!(w, "      {painted}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::schema::{DataType, StructField, StructType};
    use crate::core::model::stats::{MaintenanceInfo, TableHealth, TombstoneInfo};

    fn sample_stats() -> TableStats {
        TableStats {
            current_version: 142,
            created_timestamp: Some(i64::MAX), // invalid timestamp must NOT panic!
            last_modified_timestamp: Some(1_700_000_000_000),
            min_reader_version: 3,
            min_writer_version: 7,
            reader_features: vec!["deletionVectors".to_string()],
            writer_features: vec!["deletionVectors".to_string()],
            partition_columns: vec![],
            partition_count: 0,
            empty_partition_count: 0,
            schema_column_count: 2,
            schema_change_count: 1,
            health: TableHealth {
                total_files: 10,
                total_size: 10_000,
                avg_file_size: 1_000,
                min_file_size: 10,
                max_file_size: 9_000,
                small_file_count: 10,
                small_file_pct: 100.0,
                skew_score: 0.9,
                deletion_vector_files: 1,
                deleted_rows_in_dvs: 5,
            },
            tombstones: TombstoneInfo {
                active_tombstones: 2,
                reclaimable_bytes: 2048,
                oldest_tombstone_timestamp: Some(1_700_000_000_000),
                expired_tombstones: 0,
                expired_reclaimable_bytes: 0,
                retention_duration_ms: crate::core::model::stats::DEFAULT_DELETED_FILE_RETENTION_MS,
            },
            maintenance: MaintenanceInfo {
                last_vacuum_version: Some(120),
                last_vacuum_timestamp: Some(1_700_000_000_000),
                last_optimize_version: Some(130),
                last_optimize_timestamp: Some(1_700_000_000_000),
                last_checkpoint_version: Some(140),
                z_order_columns: vec![],
                clustering_columns: vec!["id".to_string()],
            },
            warnings: vec![],
        }
    }

    #[test]
    fn test_plain_mode_emits_zero_ansi_and_prints_issue_count_and_utc_dates() {
        let renderer = TableRenderer::with_options(true, false, true);
        let mut buf = Vec::new();
        renderer
            .render_inspect_to(&mut buf, "./tbl", &sample_stats())
            .unwrap();
        let out = String::from_utf8(buf).unwrap();

        assert!(
            !out.contains('\x1b'),
            "Plain mode must emit ZERO ANSI escape codes, got:\n{out}"
        );
        assert!(out.contains("2 issues detected:"));
        assert!(out.contains("invalid timestamp"));
        assert!(out.contains("2023-11-14 22:13:20 UTC"));
        assert!(out.contains("(none)"));
        assert!(out.contains("Clustering Columns"));
    }

    #[test]
    fn test_audit_negative_byte_delta_colored_red_and_no_panic_on_min_i64() {
        assert!(format_signed_bytes(i64::MIN).starts_with('-'));

        let entry = LineageEntry {
            version: 5,
            timestamp: Some(i64::MIN), // invalid timestamp must not panic
            operation: OperationType::Delete,
            operation_raw: "DELETE".to_string(),
            writer: "spark".to_string(),
            num_files_added: 0,
            num_files_removed: 1,
            bytes_added: 0,
            bytes_removed: 4096,
            physical_files_added: 0,
            physical_files_removed: 1,
            physical_bytes_added: 0,
            physical_bytes_removed: 4096,
            num_output_rows: None,
            predicate: Some("id = 1".to_string()),
            user_name: None,
            cluster_id: None,
            engine_info: None,
            has_commit_info: true,
        };

        let renderer = TableRenderer::with_options(false, false, true);
        colored::control::set_override(true);
        let mut buf = Vec::new();
        renderer
            .render_audit_to(&mut buf, "./tbl", &[entry])
            .unwrap();
        colored::control::unset_override();
        let out = String::from_utf8(buf).unwrap();

        // Red ANSI code (31) should be used for negative byte delta, not green (32)
        assert!(out.contains("\x1b[31m-4.1 KB") || out.contains("\x1b[31m-4.1 kB"));
        assert!(out.contains("invalid timestamp"));
    }

    #[test]
    fn test_schema_snapshot_four_column_header_and_no_header() {
        let snap = SchemaSnapshot {
            version: 3,
            timestamp: None,
            schema: StructType {
                type_name: "struct".to_string(),
                fields: vec![StructField {
                    name: "id".to_string(),
                    data_type: DataType::Primitive("long".to_string()),
                    nullable: false,
                    metadata: serde_json::json!({}),
                }],
            },
        };

        let with_hdr = TableRenderer::with_options(true, false, false);
        let mut b1 = Vec::new();
        with_hdr
            .render_schema_snapshot_to(&mut b1, "./tbl", &snap)
            .unwrap();
        let s1 = String::from_utf8(b1).unwrap();
        assert!(s1.contains("Schema at v003"));
        assert!(s1.contains("Field Name"));
        assert!(s1.contains("Metadata"));
        assert!(!s1.contains('\x1b'));

        let no_hdr = TableRenderer::with_options(true, true, false);
        let mut b2 = Vec::new();
        no_hdr
            .render_schema_snapshot_to(&mut b2, "./tbl", &snap)
            .unwrap();
        let s2 = String::from_utf8(b2).unwrap();
        assert!(!s2.contains("Schema at v003"));
        assert!(!s2.contains("Field Name"));
        assert!(s2.contains("long"));
    }

    #[test]
    fn test_no_color_follows_spec_empty_vs_non_empty() {
        use std::ffi::OsStr;

        // Per no-color.org, empty NO_COLOR="" MUST NOT disable color!
        assert!(should_use_color(false, None, true));
        assert!(should_use_color(false, Some(OsStr::new("")), true));
        // Non-empty NO_COLOR="1" disables color
        assert!(!should_use_color(false, Some(OsStr::new("1")), true));
        // Plain mode or non-TTY always disables color
        assert!(!should_use_color(true, Some(OsStr::new("")), true));
        assert!(!should_use_color(false, Some(OsStr::new("")), false));
    }

    #[test]
    fn test_checkpoint_horizon_renders_not_observed_instead_of_never() {
        let mut stats = sample_stats();
        stats.maintenance.last_vacuum_version = None;
        stats.maintenance.last_vacuum_timestamp = None;
        stats.maintenance.last_optimize_version = None;
        stats.maintenance.last_optimize_timestamp = None;
        stats.maintenance.last_checkpoint_version = Some(140);

        let renderer = TableRenderer::with_options(true, false, false);
        let mut buf = Vec::new();
        renderer
            .render_inspect_to(&mut buf, "./tbl", &stats)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Not observed since checkpoint v140"));
        assert!(!out.contains("Never"));
    }

    #[test]
    fn test_renderer_saturating_sub_on_extreme_deltas() {
        let diff = VersionDiff {
            v1: 0,
            v2: 1,
            schema_changes: vec![],
            has_breaking_schema_changes: false,
            files_added: i64::MAX,
            files_removed: i64::MIN,
            bytes_added: i64::MAX,
            bytes_removed: i64::MIN,
            physical_files_added: i64::MAX,
            physical_files_removed: -1,
            physical_bytes_added: i64::MAX,
            physical_bytes_removed: i64::MIN,
            new_partitions: vec![],
            removed_partitions: vec![],
            operations: std::collections::HashMap::new(),
            total_commits: 1,
        };
        let renderer = TableRenderer::with_options(true, false, false);
        let mut buf = Vec::new();
        renderer
            .render_diff_to(&mut buf, "./tbl", &diff, false, false)
            .unwrap();
        assert!(!buf.is_empty());

        let entry = LineageEntry {
            version: 1,
            timestamp: Some(1_700_000_000_000),
            operation: OperationType::Write,
            operation_raw: "WRITE".to_string(),
            writer: "spark".to_string(),
            num_files_added: 1,
            num_files_removed: 1,
            bytes_added: i64::MAX,
            bytes_removed: i64::MIN,
            physical_files_added: 1,
            physical_files_removed: 1,
            physical_bytes_added: i64::MIN,
            physical_bytes_removed: i64::MAX,
            num_output_rows: None,
            predicate: None,
            user_name: None,
            cluster_id: None,
            engine_info: None,
            has_commit_info: true,
        };
        let mut lin_buf = Vec::new();
        renderer
            .render_lineage_to(&mut lin_buf, "./tbl", std::slice::from_ref(&entry))
            .unwrap();
        assert!(!lin_buf.is_empty());

        let mut aud_buf = Vec::new();
        renderer
            .render_audit_to(&mut aud_buf, "./tbl", std::slice::from_ref(&entry))
            .unwrap();
        assert!(!aud_buf.is_empty());
    }
}
