use serde_json::Value;

pub(super) const MASK: &str = "••••••••";

pub(super) fn secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["key", "token", "secret", "auth", "password", "credential"]
        .iter()
        .any(|needle| name.contains(needle))
}

pub(super) fn value_shape_secret(text: &str) -> bool {
    text.to_ascii_lowercase().contains("bearer ")
        || text.starts_with("sk-")
        || text.starts_with("ghp_")
        || text.starts_with("github_pat_")
}

pub(super) fn contains_mask(value: &Value) -> bool {
    match value {
        Value::String(text) => text == MASK,
        Value::Array(items) => items.iter().any(contains_mask),
        Value::Object(map) => map.values().any(contains_mask),
        _ => false,
    }
}

pub(super) fn restore_masked(stored: &Value, edited: &mut Value) -> Result<(), MaskInArray> {
    if edited.as_str() == Some(MASK) {
        if stored.is_string() {
            *edited = stored.clone();
        }
        return Ok(());
    }
    if edited.as_array().is_some() && contains_mask(edited) {
        return Err(MaskInArray);
    }
    if let (Value::Object(previous), Value::Object(updated)) = (stored, edited) {
        for (name, child) in updated.iter_mut() {
            if let Some(original) = previous.get(name) {
                restore_masked(original, child)?;
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct MaskInArray;
