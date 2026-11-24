use crate::error::DeltaLensError;
use serde::Serialize;
use std::io::Write;

/// Render any serializable structure as pretty JSON to stdout.
pub fn render_json<T: Serialize>(value: &T) -> Result<(), DeltaLensError> {
    let mut stdout = std::io::stdout().lock();
    write_json(&mut stdout, value)
}

/// Write any serializable structure as pretty JSON to an arbitrary `Write` sink.
pub fn write_json<W: Write, T: Serialize>(w: &mut W, value: &T) -> Result<(), DeltaLensError> {
    let json_str = serde_json::to_string_pretty(value)?;
    writeln!(w, "{json_str}")?;
    Ok(())
}
