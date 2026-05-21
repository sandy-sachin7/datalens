use serde::Serialize;

/// Render any serializable structure as a pretty JSON string to stdout.
pub fn render_json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(json_str) => println!("{}", json_str),
        Err(e) => eprintln!("Error rendering JSON: {}", e),
    }
}
