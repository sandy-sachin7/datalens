use crate::core::model::stats::TableStats;
use bytesize::ByteSize;
use chrono::{TimeZone, Utc};
use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, Color, Table};
use std::path::Path;

pub struct TableRenderer {
    plain: bool,
}

impl TableRenderer {
    pub fn new(plain: bool) -> Self {
        Self { plain }
    }

    pub fn render_inspect(&self, path: &Path, stats: &TableStats) {
        let mut table = Table::new();
        if !self.plain {
            table
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS);
        }

        table.set_header(vec![
            Cell::new("deltalens · Table Health Report").fg(if self.plain {
                Color::Reset
            } else {
                Color::Cyan
            }),
            Cell::new(path.display().to_string()).fg(if self.plain {
                Color::Reset
            } else {
                Color::Cyan
            }),
        ]);

        table.add_row(vec!["Current Version", &stats.current_version.to_string()]);

        if let Some(created) = stats.created_timestamp {
            let dt = Utc.timestamp_millis_opt(created).unwrap();
            table.add_row(vec![
                "Created",
                &dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            ]);
        }
        if let Some(modified) = stats.last_modified_timestamp {
            let dt = Utc.timestamp_millis_opt(modified).unwrap();
            table.add_row(vec![
                "Last Modified",
                &dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            ]);
        }

        table.add_row(vec![
            "Protocol",
            &format!(
                "reader={}, writer={}",
                stats.min_reader_version, stats.min_writer_version
            ),
        ]);

        table.add_row(vec![
            Cell::new("DATA FILES").fg(if self.plain {
                Color::Reset
            } else {
                Color::Yellow
            }),
            Cell::new(""),
        ]);
        table.add_row(vec!["Total Files", &stats.health.total_files.to_string()]);
        table.add_row(vec![
            "Total Size",
            &ByteSize::b(stats.health.total_size as u64).to_string(),
        ]);
        table.add_row(vec![
            "Avg File Size",
            &ByteSize::b(stats.health.avg_file_size as u64).to_string(),
        ]);

        let skew_warn = stats.health.skew_score > 0.5;
        let mut min_cell = Cell::new(ByteSize::b(stats.health.min_file_size as u64).to_string());
        let mut max_cell = Cell::new(ByteSize::b(stats.health.max_file_size as u64).to_string());

        if skew_warn && !self.plain {
            min_cell = min_cell.fg(Color::Red);
            max_cell = max_cell.fg(Color::Red);
        }
        table.add_row(vec![Cell::new("Min File Size"), min_cell]);
        table.add_row(vec![Cell::new("Max File Size"), max_cell]);

        let small_warn = stats.health.small_file_pct > 20.0;
        let small_val = format!(
            "{} ({:.1}%)",
            stats.health.small_file_count, stats.health.small_file_pct
        );
        let mut small_cell = Cell::new(small_val);
        if small_warn && !self.plain {
            small_cell = small_cell.fg(Color::Red);
        }
        table.add_row(vec![Cell::new("Small Files (<32MB)"), small_cell]);

        let skew_val = format!("{:.2} / 1.0", stats.health.skew_score);
        let mut skew_cell = Cell::new(skew_val);
        if skew_warn && !self.plain {
            skew_cell = skew_cell.fg(Color::Red);
        }
        table.add_row(vec![Cell::new("Skew Score"), skew_cell]);

        table.add_row(vec![
            Cell::new("PARTITIONS").fg(if self.plain {
                Color::Reset
            } else {
                Color::Yellow
            }),
            Cell::new(""),
        ]);
        table.add_row(vec![
            "Partition Columns",
            &stats.partition_columns.join(", "),
        ]);
        table.add_row(vec!["Partition Count", &stats.partition_count.to_string()]);
        table.add_row(vec![
            "Empty Partitions",
            &stats.empty_partition_count.to_string(),
        ]);

        table.add_row(vec![
            Cell::new("MAINTENANCE").fg(if self.plain {
                Color::Reset
            } else {
                Color::Yellow
            }),
            Cell::new(""),
        ]);

        let last_vac = match stats.maintenance.last_vacuum_version {
            Some(v) => format!(
                "v{} ({} ts)",
                v,
                stats.maintenance.last_vacuum_timestamp.unwrap_or(0)
            ),
            None => "Never".to_string(),
        };
        table.add_row(vec!["Last VACUUM", &last_vac]);

        let last_opt = match stats.maintenance.last_optimize_version {
            Some(v) => format!(
                "v{} ({} ts)",
                v,
                stats.maintenance.last_optimize_timestamp.unwrap_or(0)
            ),
            None => "Never".to_string(),
        };
        table.add_row(vec!["Last OPTIMIZE", &last_opt]);

        let last_ckpt = match stats.maintenance.last_checkpoint_version {
            Some(v) => format!("v{}", v),
            None => "None found".to_string(),
        };
        table.add_row(vec!["Last CHECKPOINT", &last_ckpt]);
        table.add_row(vec![
            "Z-Order Columns",
            &stats.maintenance.z_order_columns.join(", "),
        ]);

        table.add_row(vec![
            Cell::new("SCHEMA").fg(if self.plain {
                Color::Reset
            } else {
                Color::Yellow
            }),
            Cell::new(""),
        ]);
        table.add_row(vec!["Column Count", &stats.schema_column_count.to_string()]);
        table.add_row(vec![
            "Schema Changes",
            &format!("{} (since inception)", stats.schema_change_count),
        ]);

        println!("{table}");

        let mut issues = 0;
        if skew_warn {
            issues += 1;
        }
        if small_warn {
            issues += 1;
        }
        if stats.maintenance.last_vacuum_version.is_none() && stats.current_version > 100 {
            issues += 1;
        }
        if stats.maintenance.last_optimize_version.is_none() && stats.current_version > 100 {
            issues += 1;
        }

        if issues > 0 {
            let mut msg = format!(
                "{} {} detected. Run `deltalens suggest <path>` for recommendations.",
                if !self.plain { "⚠ " } else { "" },
                if issues == 1 { "issue" } else { "issues" }
            );
            if !self.plain {
                use colored::Colorize;
                msg = msg.yellow().to_string();
            }
            println!("\n{}", msg);
        }
    }

    pub fn render_diff(
        &self,
        path: &Path,
        diff: &crate::core::analyzer::diff::VersionDiff,
        schema_only: bool,
        files_only: bool,
    ) {
        let mut table = Table::new();
        if !self.plain {
            table
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS);
        }

        table.set_header(vec![
            Cell::new(&format!("deltalens · Version Diff  v{} → v{}", diff.v1, diff.v2)).fg(if self.plain {
                Color::Reset
            } else {
                Color::Cyan
            }),
            Cell::new(path.display().to_string()).fg(if self.plain {
                Color::Reset
            } else {
                Color::Cyan
            }),
        ]);

        if !files_only {
            table.add_row(vec![
                Cell::new("SCHEMA CHANGES").fg(if self.plain {
                    Color::Reset
                } else {
                    Color::Yellow
                }),
                Cell::new(""),
            ]);

            if diff.schema_changes.is_empty() {
                table.add_row(vec!["No schema changes detected", ""]);
            } else {
                for change in &diff.schema_changes {
                    for col in &change.added_columns {
                        let cell_label = if self.plain { "+ added" } else { "+ added" };
                        let mut action_cell = Cell::new(cell_label);
                        if !self.plain {
                            action_cell = action_cell.fg(Color::Green);
                        }
                        table.add_row(vec![action_cell, Cell::new(col)]);
                    }
                    for col in &change.removed_columns {
                        let cell_label = if self.plain { "- removed" } else { "- removed" };
                        let mut action_cell = Cell::new(cell_label);
                        if !self.plain {
                            action_cell = action_cell.fg(Color::Red);
                        }
                        table.add_row(vec![action_cell, Cell::new(col)]);
                    }
                    for (name, old_t, new_t) in &change.modified_columns {
                        let cell_label = if self.plain { "~ modified" } else { "~ modified" };
                        let mut action_cell = Cell::new(cell_label);
                        if !self.plain {
                            action_cell = action_cell.fg(Color::Yellow);
                        }
                        table.add_row(vec![
                            action_cell,
                            Cell::new(format!("{} ({} → {})", name, old_t, new_t)),
                        ]);
                    }
                }
            }
        }

        if !schema_only {
            table.add_row(vec![
                Cell::new("DATA CHANGES").fg(if self.plain {
                    Color::Reset
                } else {
                    Color::Yellow
                }),
                Cell::new(""),
            ]);

            table.add_row(vec!["Files Added", &format!("+{}", diff.files_added)]);
            table.add_row(vec!["Files Removed", &format!("-{}", diff.files_removed)]);
            table.add_row(vec![
                "Net File Delta",
                &format!("{:+}", diff.files_added - diff.files_removed),
            ]);
            table.add_row(vec![
                "Size Added",
                &ByteSize::b(diff.bytes_added as u64).to_string(),
            ]);
            table.add_row(vec![
                "Size Removed",
                &ByteSize::b(diff.bytes_removed as u64).to_string(),
            ]);

            let net_bytes = diff.bytes_added - diff.bytes_removed;
            let net_size_str = if net_bytes >= 0 {
                format!("+{}", ByteSize::b(net_bytes as u64))
            } else {
                format!("-{}", ByteSize::b((-net_bytes) as u64))
            };
            table.add_row(vec!["Net Size Delta", &net_size_str]);

            table.add_row(vec![
                Cell::new(format!("OPERATIONS ({} commits)", diff.total_commits)).fg(if self.plain {
                    Color::Reset
                } else {
                    Color::Yellow
                }),
                Cell::new(""),
            ]);

            if diff.operations.is_empty() {
                table.add_row(vec!["No operations recorded", ""]);
            } else {
                let mut ops: Vec<_> = diff.operations.iter().collect();
                ops.sort_by_key(|(op, _)| *op);
                for (op, count) in ops {
                    table.add_row(vec![op, &count.to_string()]);
                }
            }

            table.add_row(vec![
                Cell::new("PARTITION CHANGES").fg(if self.plain {
                    Color::Reset
                } else {
                    Color::Yellow
                }),
                Cell::new(""),
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
            table.add_row(vec!["New Partitions", &new_part_str]);
            table.add_row(vec!["Removed Partitions", &rem_part_str]);
        }

        println!("{table}");
    }

    pub fn render_lineage(&self, _path: &Path, entries: &[crate::core::model::lineage::LineageEntry]) {
        if entries.is_empty() {
            println!("No lineage entries found.");
            return;
        }

        for entry in entries {
            let dt = Utc.timestamp_millis_opt(entry.timestamp).unwrap();
            let dt_str = dt.format("%Y-%m-%d %H:%M").to_string();

            let net_bytes = entry.bytes_added - entry.bytes_removed;
            let size_str = if net_bytes >= 0 {
                format!("+{}", ByteSize::b(net_bytes as u64))
            } else {
                format!("-{}", ByteSize::b((-net_bytes) as u64))
            };

            let rows_str = match entry.num_output_rows {
                Some(r) => {
                    let abs_r = r.abs();
                    let formatted_r = if abs_r >= 1_000_000 {
                        format!("{:.1}M", r as f64 / 1_000_000.0)
                    } else if abs_r >= 1_000 {
                        format!("{:.1}K", r as f64 / 1_000.0)
                    } else {
                        r.to_string()
                    };
                    if r > 0 {
                        format!("+{} rows", formatted_r)
                    } else if r < 0 {
                        format!("{} rows", formatted_r)
                    } else {
                        "±0 rows".to_string()
                    }
                }
                None => "".to_string(),
            };

            if self.plain {
                println!(
                    "v{:<4}  {:<16}  {:<12}  {:<24}  {:<8}  {}",
                    entry.version, dt_str, entry.operation_raw, entry.writer, size_str, rows_str
                );
            } else {
                use colored::Colorize;
                let version_styled = format!("v{}", entry.version).cyan();
                let dt_styled = dt_str.truecolor(128, 128, 128); // gray

                let op_styled = match entry.operation {
                    crate::core::model::lineage::OperationType::Write => entry.operation_raw.green(),
                    crate::core::model::lineage::OperationType::Merge => entry.operation_raw.blue(),
                    crate::core::model::lineage::OperationType::Delete => entry.operation_raw.red(),
                    crate::core::model::lineage::OperationType::Update => entry.operation_raw.yellow(),
                    crate::core::model::lineage::OperationType::Optimize => {
                        entry.operation_raw.magenta()
                    }
                    _ => entry.operation_raw.normal(),
                };

                let writer_styled = entry.writer.white();
                let size_styled = if net_bytes >= 0 {
                    size_str.green()
                } else {
                    size_str.red()
                };
                let rows_styled = if entry.num_output_rows.unwrap_or(0) >= 0 {
                    rows_str.green()
                } else {
                    rows_str.red()
                };

                println!(
                    "{:<14}  {:<25}  {:<22}  {:<32}  {:<18}  {}",
                    version_styled,
                    dt_styled,
                    op_styled,
                    writer_styled,
                    size_styled,
                    rows_styled
                );
            }
        }
    }

    pub fn render_audit(&self, _path: &Path, entries: &[crate::core::model::lineage::LineageEntry]) {
        if entries.is_empty() {
            println!("No audit operations found matching the criteria.");
            return;
        }

        if !self.plain {
            use colored::Colorize;
            println!("{}", "Audit Trail".cyan().bold());
        } else {
            println!("Audit Trail");
        }

        for entry in entries {
            let dt = Utc.timestamp_millis_opt(entry.timestamp).unwrap();
            let dt_str = dt.format("%Y-%m-%d %H:%M").to_string();

            let predicate_str = match &entry.predicate {
                Some(p) => format!("predicate=\"{}\"", p),
                None => "".to_string(),
            };

            let net_bytes = entry.bytes_added - entry.bytes_removed;
            let size_str = if net_bytes >= 0 {
                format!("+{}", ByteSize::b(net_bytes as u64))
            } else {
                format!("-{}", ByteSize::b((-net_bytes) as u64))
            };

            let rows_str = match entry.num_output_rows {
                Some(r) => {
                    let abs_r = r.abs();
                    let formatted_r = if abs_r >= 1_000_000 {
                        format!("{:.1}M", r as f64 / 1_000_000.0)
                    } else if abs_r >= 1_000 {
                        format!("{:.1}K", r as f64 / 1_000.0)
                    } else {
                        r.to_string()
                    };
                    if r > 0 {
                        format!("+{} rows", formatted_r)
                    } else if r < 0 {
                        format!("{} rows", formatted_r)
                    } else {
                        "±0 rows".to_string()
                    }
                }
                None => size_str,
            };

            if self.plain {
                println!(
                    "{}  {:<10}  {:<12}  {:<35}  {}",
                    dt_str, entry.operation_raw, entry.writer, predicate_str, rows_str
                );
            } else {
                use colored::Colorize;
                let dt_styled = dt_str.truecolor(128, 128, 128); // gray

                let op_styled = match entry.operation {
                    crate::core::model::lineage::OperationType::Merge => entry.operation_raw.blue(),
                    crate::core::model::lineage::OperationType::Delete => entry.operation_raw.red(),
                    crate::core::model::lineage::OperationType::Update => entry.operation_raw.yellow(),
                    _ => entry.operation_raw.normal(),
                };

                let writer_styled = entry.writer.white();
                let pred_styled = predicate_str.truecolor(140, 180, 250); // soft blue
                let rows_styled = if entry.num_output_rows.unwrap_or(0) >= 0 {
                    rows_str.green()
                } else {
                    rows_str.red()
                };

                println!(
                    "{}  {:<20}  {:<22}  {:<45}  {}",
                    dt_styled, op_styled, writer_styled, pred_styled, rows_styled
                );
            }
        }
        println!("\n{} operations found.", entries.len());
    }

    pub fn render_schema_snapshot(
        &self,
        path: &Path,
        snapshot: &crate::core::model::schema::SchemaSnapshot,
    ) {
        let mut table = Table::new();
        if !self.plain {
            table
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS);
        }

        table.set_header(vec![
            Cell::new(&format!("Schema at Version {}", snapshot.version)).fg(if self.plain {
                Color::Reset
            } else {
                Color::Cyan
            }),
            Cell::new(path.display().to_string()).fg(if self.plain {
                Color::Reset
            } else {
                Color::Cyan
            }),
        ]);

        table.add_row(vec![
            Cell::new("Field Name").fg(if self.plain { Color::Reset } else { Color::Yellow }),
            Cell::new("Type").fg(if self.plain { Color::Reset } else { Color::Yellow }),
            Cell::new("Nullable").fg(if self.plain { Color::Reset } else { Color::Yellow }),
            Cell::new("Metadata").fg(if self.plain { Color::Reset } else { Color::Yellow }),
        ]);

        for field in &snapshot.schema.fields {
            let nullable_str = if field.nullable { "true" } else { "false" };

            let type_str = if field.data_type.is_string() {
                field.data_type.as_str().unwrap().to_string().to_uppercase()
            } else {
                field.data_type.to_string()
            };

            let meta_str = if field.metadata.is_null()
                || (field.metadata.is_object() && field.metadata.as_object().unwrap().is_empty())
            {
                "-".to_string()
            } else {
                field.metadata.to_string()
            };

            table.add_row(vec![&field.name, &type_str, nullable_str, &meta_str]);
        }

        println!("{table}");
    }

    pub fn render_schema_history(
        &self,
        path: &Path,
        history: &[crate::core::model::schema::SchemaChange],
    ) {
        if history.is_empty() {
            println!("No schema evolution history detected.");
            return;
        }

        if !self.plain {
            use colored::Colorize;
            println!(
                "{}",
                format!("Schema Evolution · {}", path.display())
                    .cyan()
                    .bold()
            );
            println!();
        } else {
            println!("Schema Evolution · {}", path.display());
            println!();
        }

        for change in history {
            let dt_str = match change.timestamp {
                Some(ts) => {
                    let dt = Utc.timestamp_millis_opt(ts).unwrap();
                    dt.format("%Y-%m-%d %H:%M").to_string()
                }
                None => "N/A".to_string(),
            };

            if self.plain {
                println!("v{:<3}  {}  Schema Change", change.version, dt_str);
                for col in &change.added_columns {
                    println!("      + added: {}", col);
                }
                for col in &change.removed_columns {
                    println!("      - removed: {}", col);
                }
                for (name, old_t, new_t) in &change.modified_columns {
                    println!("      ~ modified: {} ({} → {})", name, old_t, new_t);
                }
            } else {
                use colored::Colorize;
                let version_styled = format!("v{:03}", change.version).cyan();
                let dt_styled = dt_str.truecolor(128, 128, 128); // gray

                println!("{}  {}  Schema Change", version_styled, dt_styled);
                for col in &change.added_columns {
                    println!("      {}", format!("+ added: {}", col).green());
                }
                for col in &change.removed_columns {
                    println!("      {}", format!("- removed: {}", col).red());
                }
                for (name, old_t, new_t) in &change.modified_columns {
                    println!(
                        "      {}",
                        format!("~ modified: {} ({} → {})", name, old_t, new_t).yellow()
                    );
                }
            }
        }
    }
}
