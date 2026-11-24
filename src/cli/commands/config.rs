use crate::cli::output_opts::{CommandOutcome, OutputOpts};
use crate::core::config::DeltalensConfig;
use crate::core::metrics::MetricsRecorder;
use crate::error::DeltaLensError;
use crate::render::json::render_json;

pub fn show(opts: OutputOpts) -> Result<CommandOutcome, DeltaLensError> {
    let config = DeltalensConfig::load();
    let cfg_path = DeltalensConfig::path();
    if opts.json {
        render_json(&serde_json::json!({
            "path": cfg_path.display().to_string(),
            "telemetry": config.telemetry,
        }))?;
    } else {
        println!("Config file: {}", cfg_path.display());
        println!(
            "Telemetry: {}",
            if config.telemetry {
                "enabled"
            } else {
                "disabled"
            }
        );
    }
    Ok(CommandOutcome::default())
}

pub fn path(opts: OutputOpts) -> Result<CommandOutcome, DeltaLensError> {
    let cfg_path = DeltalensConfig::path();
    if opts.json {
        render_json(&serde_json::json!({
            "path": cfg_path.display().to_string(),
        }))?;
    } else {
        println!("{}", cfg_path.display());
    }
    Ok(CommandOutcome::default())
}

pub fn set(key: &str, value: &str, opts: OutputOpts) -> Result<CommandOutcome, DeltaLensError> {
    let mut config = DeltalensConfig::load();
    match key {
        "telemetry" => match value.to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => config.telemetry = true,
            "false" | "no" | "0" => config.telemetry = false,
            _ => {
                return Err(DeltaLensError::InvalidInput(format!(
                    "Invalid value '{value}' for '{key}'. Expected true or false."
                )));
            }
        },
        _ => {
            return Err(DeltaLensError::InvalidInput(format!(
                "Unknown config key '{key}'. Available keys: telemetry"
            )));
        }
    }

    config
        .save()
        .map_err(|e| DeltaLensError::Storage(e.to_string()))?;

    if opts.json {
        render_json(&serde_json::json!({
            "key": key,
            "value": config.telemetry,
            "path": DeltalensConfig::path().display().to_string(),
        }))?;
    } else {
        println!("Set {} = {}", key, config.telemetry);
    }
    Ok(CommandOutcome::default())
}

pub fn metrics(opts: OutputOpts) -> Result<CommandOutcome, DeltaLensError> {
    let summary = MetricsRecorder::summary();
    if opts.json {
        render_json(&summary)?;
        return Ok(CommandOutcome::default());
    }

    if !opts.no_header {
        println!("=== Metrics Summary ===");
    }
    println!("Total commands:     {}", summary.total_commands);
    if summary.total_commands == 0 {
        println!("No metrics recorded yet. Enable telemetry with:");
        println!("  deltalens config set telemetry true");
        return Ok(CommandOutcome::default());
    }
    println!("Total duration:     {:.1} ms", summary.total_duration_ms);
    println!("Average duration:   {:.1} ms", summary.avg_duration_ms);
    println!("P50 latency:        {:.1} ms", summary.p50_ms);
    println!("P95 latency:        {:.1} ms", summary.p95_ms);
    println!("P99 latency:        {:.1} ms", summary.p99_ms);
    println!();
    if !opts.no_header {
        println!("=== By Command ===");
    }
    let mut pairs: Vec<_> = summary.command_counts.iter().collect();
    pairs.sort_by(|(a_cmd, &a_cnt), (b_cmd, &b_cnt)| {
        b_cnt.cmp(&a_cnt).then_with(|| a_cmd.cmp(b_cmd))
    });
    for (cmd, count) in &pairs {
        println!("  {:<12} {}", cmd, count);
    }
    Ok(CommandOutcome::default())
}
