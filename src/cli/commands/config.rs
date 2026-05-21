use crate::core::config::DeltalensConfig;
use crate::core::metrics::MetricsRecorder;
use crate::error::DeltaLensError;

pub fn show() -> Result<(), DeltaLensError> {
    let config = DeltalensConfig::load();
    println!("Config file: {}", DeltalensConfig::path().display());
    println!(
        "Telemetry: {}",
        if config.telemetry {
            "enabled"
        } else {
            "disabled"
        }
    );
    Ok(())
}

pub fn path() -> Result<(), DeltaLensError> {
    println!("{}", DeltalensConfig::path().display());
    Ok(())
}

pub fn set(key: &str, value: &str) -> Result<(), DeltaLensError> {
    let mut config = DeltalensConfig::load();
    match key {
        "telemetry" => match value {
            "true" | "yes" | "1" => config.telemetry = true,
            "false" | "no" | "0" => config.telemetry = false,
            _ => {
                eprintln!(
                    "Invalid value '{}' for '{}'. Use true or false.",
                    value, key
                );
                return Ok(());
            }
        },
        _ => {
            eprintln!("Unknown config key '{}'. Available keys: telemetry", key);
        }
    }
    config
        .save()
        .map_err(|e| DeltaLensError::Storage(e.to_string()))?;
    println!("Set {} = {}", key, value);
    Ok(())
}

pub fn metrics() -> Result<(), DeltaLensError> {
    let summary = MetricsRecorder::summary();
    println!("=== Metrics Summary ===");
    println!("Total commands:     {}", summary.total_commands);
    if summary.total_commands == 0 {
        println!("No metrics recorded yet. Enable telemetry with:");
        println!("  deltalens config set telemetry true");
        return Ok(());
    }
    println!("Total duration:     {:.1} ms", summary.total_duration_ms);
    println!("Average duration:   {:.1} ms", summary.avg_duration_ms);
    println!("P50 latency:        {:.1} ms", summary.p50_ms);
    println!("P95 latency:        {:.1} ms", summary.p95_ms);
    println!("P99 latency:        {:.1} ms", summary.p99_ms);
    println!();
    println!("=== By Command ===");
    let mut pairs: Vec<_> = summary.command_counts.iter().collect();
    pairs.sort_by_key(|(_, &c)| std::cmp::Reverse(c));
    for (cmd, count) in &pairs {
        println!("  {:<12} {}", cmd, count);
    }
    Ok(())
}
