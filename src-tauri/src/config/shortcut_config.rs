//! Shortcut-specific raw JSON handling: schema version parsing, Windows schema
//! v1 migration and field-level helpers.
//!
//! All functions operate on `serde_json::Value` so that unknown fields and the
//! original representation of untouched values are preserved. Only the four
//! shortcut fields and (on Windows) the shortcut schema marker are ever
//! rewritten.

use serde_json::{Map, Value};

use crate::shortcut::model::{
    self, build_conflict_graph, NormalizedShortcut, ShortcutField,
};

/// Highest Windows shortcut-schema version this build understands.
pub const SUPPORTED_WINDOWS_SHORTCUT_SCHEMA_VERSION: u64 = 1;
/// On-disk key for the shortcut-only schema version.
pub const SCHEMA_VERSION_KEY: &str = "windowsShortcutSchemaVersion";

/// Raw schema marker as found on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawSchemaVersion {
    Missing,
    Version(u64),
    /// Non-integer, negative or otherwise not safely interpretable.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutSchemaMode {
    /// Schema is understood and automatic (shortcut-only) maintenance is safe.
    Supported,
    /// Schema is newer than this build or of an unknown type: never auto-write.
    Future,
}

pub fn parse_schema_version(raw: &Value) -> RawSchemaVersion {
    let Some(value) = raw
        .get("general")
        .and_then(|general| general.get(SCHEMA_VERSION_KEY))
    else {
        return RawSchemaVersion::Missing;
    };
    match value {
        Value::Null => RawSchemaVersion::Missing,
        Value::Number(number) => match number.as_u64() {
            Some(version) => RawSchemaVersion::Version(version),
            None => RawSchemaVersion::Unsupported,
        },
        _ => RawSchemaVersion::Unsupported,
    }
}

pub fn schema_mode(raw: &Value) -> ShortcutSchemaMode {
    match parse_schema_version(raw) {
        RawSchemaVersion::Missing => ShortcutSchemaMode::Supported,
        RawSchemaVersion::Version(version)
            if version <= SUPPORTED_WINDOWS_SHORTCUT_SCHEMA_VERSION =>
        {
            ShortcutSchemaMode::Supported
        }
        RawSchemaVersion::Version(_) | RawSchemaVersion::Unsupported => ShortcutSchemaMode::Future,
    }
}

/// Platform default for one field.
///
/// Only Windows changes the voice shortcut default to the physical right Alt;
/// every other platform keeps the existing `F4`.
pub fn platform_default(field: ShortcutField, is_windows: bool) -> &'static str {
    match (field, is_windows) {
        (ShortcutField::Shortcut, true) => model::ALT_RIGHT,
        (ShortcutField::Shortcut, false) => "F4",
        (ShortcutField::Shortcut2, _) => "",
        (ShortcutField::ExtractShortcut, _) => "F6",
        (ShortcutField::ExtractShortcut2, _) => "",
    }
}

/// Read the raw JSON value for a field, or `None` when absent.
pub fn raw_field<'a>(raw: &'a Value, field: ShortcutField) -> Option<&'a Value> {
    raw.get("general").and_then(|general| general.get(field.key()))
}

/// Normalize the current raw value of a field for conflict/validation purposes.
/// Non-string values are treated as invalid (never registered) without being
/// rewritten.
pub fn normalized_field(raw: &Value, field: ShortcutField) -> NormalizedShortcut {
    match raw_field(raw, field) {
        None => NormalizedShortcut::Empty,
        Some(Value::Null) => NormalizedShortcut::Empty,
        Some(Value::String(value)) => model::normalize(value),
        Some(_) => NormalizedShortcut::Invalid("<非字符串值>".to_string()),
    }
}

pub fn normalized_fields(raw: &Value) -> [NormalizedShortcut; 4] {
    [
        normalized_field(raw, ShortcutField::Shortcut),
        normalized_field(raw, ShortcutField::Shortcut2),
        normalized_field(raw, ShortcutField::ExtractShortcut),
        normalized_field(raw, ShortcutField::ExtractShortcut2),
    ]
}

/// Ensure `raw` is an object and `raw["general"]` is an object, returning the
/// general map.
pub fn ensure_general_object(raw: &mut Value) -> Result<&mut Map<String, Value>, String> {
    if !raw.is_object() {
        *raw = Value::Object(Map::new());
    }
    let root = raw.as_object_mut().expect("just ensured object");
    let general = root
        .entry("general".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !general.is_object() {
        *general = Value::Object(Map::new());
    }
    Ok(general.as_object_mut().expect("just ensured object"))
}

/// Write a field value as a JSON string. Preserves everything else.
pub fn set_field(raw: &mut Value, field: ShortcutField, value: &str) -> Result<(), String> {
    let general = ensure_general_object(raw)?;
    general.insert(field.key().to_string(), Value::String(value.to_string()));
    Ok(())
}

/// Deep-merge `incoming` into `target`. Objects merge recursively; arrays and
/// scalars are replaced. Unknown keys already present in `target` survive.
fn merge_value(target: &mut Value, incoming: Value) {
    match (target, incoming) {
        (Value::Object(target_map), Value::Object(incoming_map)) => {
            for (key, value) in incoming_map {
                match target_map.get_mut(&key) {
                    Some(slot) => merge_value(slot, value),
                    None => {
                        target_map.insert(key, value);
                    }
                }
            }
        }
        (slot, replacement) => *slot = replacement,
    }
}

/// All keys within `general` that belong to the shortcut domain and must be
/// protected from being overwritten by general config snapshots.
pub const SHORTCUT_DOMAIN_KEYS: &[&str] = &[
    SCHEMA_VERSION_KEY,
    "shortcut",
    "shortcut2",
    "extractShortcut",
    "extractShortcut2",
    "shortcutTemplate",
    "shortcut2Template",
    "extractShortcutTemplate",
    "extractShortcut2Template",
    "textStrengtheningEnabled",
];

/// A sparse persistence bundle for the shortcut domain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortcutDomainPersist {
    pub keys: Vec<(ShortcutField, String)>,
    pub templates: Vec<(ShortcutField, String)>,
    pub text_strengthening_enabled: Option<bool>,
}

impl ShortcutDomainPersist {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
            && self.templates.is_empty()
            && self.text_strengthening_enabled.is_none()
    }
}

/// Apply a shortcut domain persistence bundle directly to the raw JSON Value.
pub fn apply_shortcut_domain_persist(
    raw: &mut Value,
    persist: &ShortcutDomainPersist,
) -> Result<(), String> {
    let general = ensure_general_object(raw)?;
    for (field, value) in &persist.keys {
        general.insert(field.key().to_string(), Value::String(value.clone()));
    }
    for (field, value) in &persist.templates {
        let template_key = match field {
            ShortcutField::Shortcut => "shortcutTemplate",
            ShortcutField::Shortcut2 => "shortcut2Template",
            ShortcutField::ExtractShortcut => "extractShortcutTemplate",
            ShortcutField::ExtractShortcut2 => "extractShortcut2Template",
        };
        general.insert(template_key.to_string(), Value::String(value.clone()));
    }
    if let Some(enabled) = persist.text_strengthening_enabled {
        general.insert("textStrengtheningEnabled".to_string(), Value::Bool(enabled));
    }
    Ok(())
}

/// Merge a serialized `AppConfig` into the latest raw JSON.
///
/// All shortcut-domain fields (keys, templates, textStrengtheningEnabled and
/// schema version) are *always* preserved from `raw` (the authoritative latest
/// persisted state); they are only ever changed through the shortcut-specific
/// update path. This prevents a stale frontend `AppConfig` snapshot from
/// overwriting shortcuts or downgrading a higher schema.
pub fn merge_config_preserving_shortcuts(raw: &mut Value, incoming: Value) {
    let protected: Vec<(&str, Option<Value>)> = SHORTCUT_DOMAIN_KEYS
        .iter()
        .map(|&key| {
            let value = raw
                .get("general")
                .and_then(|general| general.get(key))
                .cloned();
            (key, value)
        })
        .collect();

    merge_value(raw, incoming);

    if let Ok(general) = ensure_general_object(raw) {
        for (key, value) in protected {
            match value {
                Some(value) => {
                    general.insert(key.to_string(), value);
                }
                None => {
                    general.remove(key);
                }
            }
        }
    }
}

/// Produce a `deserialize`-friendly copy where non-string shortcut values are
/// replaced by `""` so a single malformed field cannot reset the whole config.
pub fn sanitized_for_deserialize(raw: &Value) -> Value {
    let mut sanitized = raw.clone();
    if let Ok(general) = ensure_general_object(&mut sanitized) {
        for field in ShortcutField::ALL {
            if let Some(value) = general.get(field.key()) {
                if !value.is_string() {
                    general.insert(field.key().to_string(), Value::String(String::new()));
                }
            }
        }
    }
    sanitized
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortcutMigrationOutcome {
    /// The candidate differs from the input and (when supported) should be
    /// persisted.
    pub changed: bool,
    /// Schema v1 check finished; on Windows the marker is set to `1`.
    pub schema_completed: bool,
    /// The disk schema is newer/unknown; nothing may be auto-written.
    pub future_mode: bool,
    pub f4_replaced: bool,
    pub f4_kept_conflict: bool,
    pub diagnostics: Vec<String>,
}

/// Run the shortcut part of raw JSON migration.
///
/// `schema_lt_1` must be computed from the *original* schema marker before this
/// call. When `is_windows` is false the Windows schema marker is never created
/// or advanced.
pub fn migrate_shortcuts(
    raw: &mut Value,
    is_windows: bool,
    schema_lt_1: bool,
) -> ShortcutMigrationOutcome {
    let mut outcome = ShortcutMigrationOutcome::default();

    if schema_mode(raw) == ShortcutSchemaMode::Future {
        outcome.future_mode = true;
        return outcome;
    }

    if let Err(error) = ensure_general_object(raw) {
        outcome.diagnostics.push(error);
        return outcome;
    }

    // 1. Normalize missing / null / blank / Backspace for the four fields.
    for field in ShortcutField::ALL {
        let existing = raw_field(raw, field).cloned();
        match existing {
            None => {
                let default = platform_default(field, is_windows);
                let _ = set_field(raw, field, default);
                outcome.changed = true;
                outcome
                    .diagnostics
                    .push(format!("字段 {} 缺失，补全为 {}", field.key(), value_label(default)));
            }
            Some(Value::Null) => {
                let _ = set_field(raw, field, "");
                outcome.changed = true;
                outcome
                    .diagnostics
                    .push(format!("字段 {} 为 null，规范化为空", field.key()));
            }
            Some(Value::String(value)) => match model::normalize(&value) {
                NormalizedShortcut::Empty => {
                    if !value.is_empty() {
                        let _ = set_field(raw, field, "");
                        outcome.changed = true;
                        outcome.diagnostics.push(format!(
                            "字段 {} 的空白或 Backspace 值规范化为空",
                            field.key()
                        ));
                    }
                }
                NormalizedShortcut::Valid(_) => {
                    // Preserve valid user values verbatim.
                }
                NormalizedShortcut::Invalid(raw_value) => {
                    // Preserve the original value; only record a diagnostic.
                    outcome.diagnostics.push(format!(
                        "字段 {} 含无法识别的值 {}，运行时将跳过注册",
                        field.key(),
                        value_label(&raw_value)
                    ));
                }
            },
            Some(other) => {
                outcome.diagnostics.push(format!(
                    "字段 {} 为非字符串值 {}，保留原始 JSON 并跳过注册",
                    field.key(),
                    other
                ));
            }
        }
    }

    // 2. Windows schema v1: F4 -> AltRight, checked against the full candidate.
    if is_windows && schema_lt_1 {
        let shortcut = normalized_field(raw, ShortcutField::Shortcut);
        if shortcut.canonical() == Some("F4") {
            let mut candidate = normalized_fields(raw);
            candidate[ShortcutField::Shortcut.index()] =
                NormalizedShortcut::Valid(model::ALT_RIGHT.to_string());
            let strengthening_enabled = raw
                .get("general")
                .and_then(|g| g.get("textStrengtheningEnabled"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let active = [true, strengthening_enabled, true, true];
            let conflict_candidate = model::effective_conflict_candidates(&candidate, &active);
            if build_conflict_graph(&conflict_candidate).has_conflicts() {
                outcome.f4_kept_conflict = true;
                outcome.diagnostics.push(
                    "F4 → AltRight 会产生冲突，保留 F4 并保留其他用户快捷键".to_string(),
                );
            } else {
                let _ = set_field(raw, ShortcutField::Shortcut, model::ALT_RIGHT);
                outcome.changed = true;
                outcome.f4_replaced = true;
            }
        }
    }

    // 3. Advance the Windows schema marker only after a completed check.
    if is_windows && schema_lt_1 {
        if let Ok(general) = ensure_general_object(raw) {
            general.insert(
                SCHEMA_VERSION_KEY.to_string(),
                Value::Number(SUPPORTED_WINDOWS_SHORTCUT_SCHEMA_VERSION.into()),
            );
            outcome.changed = true;
            outcome.schema_completed = true;
        }
    }

    outcome
}

fn value_label(value: &str) -> String {
    if value.is_empty() {
        "空".to_string()
    } else {
        format!("\"{}\"", value)
    }
}

/// One-line summary of the on-disk shortcut state, for diagnostics.
pub fn raw_shortcut_summary(raw: &Value) -> String {
    let mut parts = Vec::new();
    for field in ShortcutField::ALL {
        let value = raw
            .get("general")
            .and_then(|general| general.get(field.key()));
        parts.push(format!(
            "{}={}",
            field.key(),
            value
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<missing>".to_string())
        ));
    }
    format!(
        "schema={:?} {}",
        parse_schema_version(raw),
        parts.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn general_shortcut(raw: &Value) -> Option<&str> {
        raw.get("general")
            .and_then(|g| g.get("shortcut"))
            .and_then(|v| v.as_str())
    }

    fn schema(raw: &Value) -> Option<u64> {
        raw.get("general")
            .and_then(|g| g.get(SCHEMA_VERSION_KEY))
            .and_then(|v| v.as_u64())
    }

    #[test]
    fn parses_missing_and_null_schema_as_version_zero() {
        assert_eq!(parse_schema_version(&json!({})), RawSchemaVersion::Missing);
        assert_eq!(
            parse_schema_version(&json!({ "general": { SCHEMA_VERSION_KEY: null } })),
            RawSchemaVersion::Missing
        );
        assert_eq!(
            parse_schema_version(&json!({ "general": { SCHEMA_VERSION_KEY: 1 } })),
            RawSchemaVersion::Version(1)
        );
        assert_eq!(
            parse_schema_version(&json!({ "general": { SCHEMA_VERSION_KEY: "x" } })),
            RawSchemaVersion::Unsupported
        );
    }

    #[test]
    fn m01_windows_new_config_defaults_to_alt_right() {
        let mut raw = json!({ "general": {} });
        let outcome = migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some("AltRight"));
        assert_eq!(schema(&raw), Some(1));
        assert!(outcome.changed);
    }

    #[test]
    fn m02_f4_without_conflict_becomes_alt_right() {
        let mut raw = json!({ "general": { "shortcut": "F4", "extractShortcut": "F6" } });
        let outcome = migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some("AltRight"));
        assert_eq!(schema(&raw), Some(1));
        assert!(outcome.f4_replaced);
    }

    #[test]
    fn m03_f4_with_alt_combo_is_kept() {
        let mut raw = json!({ "general": { "shortcut": "F4", "shortcut2": "Alt+A" } });
        let outcome = migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some("F4"));
        assert_eq!(raw["general"]["shortcut2"], "Alt+A");
        assert_eq!(schema(&raw), Some(1));
        assert!(outcome.f4_kept_conflict);
    }

    #[test]
    fn m04_f4_with_existing_alt_right_is_kept() {
        let mut raw = json!({ "general": { "shortcut": "F4", "shortcut2": "AltRight" } });
        let outcome = migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some("F4"));
        assert_eq!(schema(&raw), Some(1));
        assert!(outcome.f4_kept_conflict);
    }

    #[test]
    fn m05_f8_is_preserved() {
        let mut raw = json!({ "general": { "shortcut": "F8" } });
        let outcome = migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some("F8"));
        assert_eq!(schema(&raw), Some(1));
        assert!(!outcome.f4_replaced);
    }

    #[test]
    fn m06_alt_right_is_preserved() {
        let mut raw = json!({ "general": { "shortcut": "AltRight" } });
        migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some("AltRight"));
        assert_eq!(schema(&raw), Some(1));
    }

    #[test]
    fn m07_empty_is_preserved() {
        let mut raw = json!({ "general": { "shortcut": "" } });
        migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some(""));
        assert_eq!(schema(&raw), Some(1));
    }

    #[test]
    fn m08_null_becomes_empty() {
        let mut raw = json!({ "general": { "shortcut": null } });
        migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some(""));
    }

    #[test]
    fn m09_blank_becomes_empty() {
        let mut raw = json!({ "general": { "shortcut": "   " } });
        migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some(""));
    }

    #[test]
    fn m10_backspace_becomes_empty() {
        for value in ["Backspace", "Ctrl+Backspace", "Alt+Backspace"] {
            let mut raw = json!({ "general": { "shortcut": value } });
            migrate_shortcuts(&mut raw, true, true);
            assert_eq!(general_shortcut(&raw), Some(""), "value={value}");
        }
    }

    #[test]
    fn m11_unknown_value_is_preserved() {
        let mut raw = json!({ "general": { "shortcut": "NotAKey" } });
        migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some("NotAKey"));
        assert_eq!(schema(&raw), Some(1));
    }

    #[test]
    fn m12_empty_extract_shortcut_is_never_restored() {
        let mut raw = json!({ "general": { "extractShortcut": "" } });
        migrate_shortcuts(&mut raw, true, true);
        assert_eq!(raw["general"]["extractShortcut"], "");
    }

    #[test]
    fn m13_missing_extract_shortcut_defaults_to_f6() {
        let mut raw = json!({ "general": {} });
        migrate_shortcuts(&mut raw, true, true);
        assert_eq!(raw["general"]["extractShortcut"], "F6");
    }

    #[test]
    fn m16_schema_one_f4_is_not_migrated() {
        let mut raw = json!({ "general": { "shortcut": "F4", SCHEMA_VERSION_KEY: 1 } });
        let outcome = migrate_shortcuts(&mut raw, true, false);
        assert_eq!(general_shortcut(&raw), Some("F4"));
        assert!(!outcome.f4_replaced);
    }

    #[test]
    fn m17_future_schema_is_untouched() {
        let mut raw = json!({ "general": { "shortcut": "F4", SCHEMA_VERSION_KEY: 2 } });
        let before = raw.clone();
        let outcome = migrate_shortcuts(&mut raw, true, false);
        assert!(outcome.future_mode);
        assert_eq!(raw, before);
    }

    #[test]
    fn m18_non_windows_does_not_touch_schema() {
        let mut raw = json!({ "general": { "shortcut": "F4" } });
        let outcome = migrate_shortcuts(&mut raw, false, false);
        assert_eq!(general_shortcut(&raw), Some("F4"));
        assert_eq!(schema(&raw), None);
        assert!(!outcome.schema_completed);
    }

    #[test]
    fn non_string_value_is_preserved_but_sanitized_for_deserialize() {
        let mut raw = json!({ "general": { "shortcut": ["oops"] } });
        let outcome = migrate_shortcuts(&mut raw, true, true);
        assert_eq!(raw["general"]["shortcut"], json!(["oops"]));
        assert!(outcome.diagnostics.iter().any(|d| d.contains("非字符串")));
        let sanitized = sanitized_for_deserialize(&raw);
        assert_eq!(sanitized["general"]["shortcut"], "");
    }

    #[test]
    fn unknown_fields_survive_merge() {
        let mut raw = json!({
            "general": { "shortcut": "AltRight", SCHEMA_VERSION_KEY: 1, "futureField": 7 },
            "unknownSection": { "keep": true }
        });
        let incoming = json!({
            "general": { "shortcut": "F4", "launchAtLogin": true },
            "localApi": { "enabled": false, "port": 8765 }
        });
        merge_config_preserving_shortcuts(&mut raw, incoming);
        assert_eq!(general_shortcut(&raw), Some("AltRight"));
        assert_eq!(schema(&raw), Some(1));
        assert_eq!(raw["general"]["futureField"], 7);
        assert_eq!(raw["unknownSection"]["keep"], true);
        assert_eq!(raw["general"]["launchAtLogin"], true);
    }

    #[test]
    fn stored_shortcut2_survives_disable_update_restart_reenable() {
        // Initial state with shortcut2 configured and strengthening enabled
        let mut raw = json!({
            "general": {
                "shortcut": "AltRight",
                "shortcut2": "F8",
                "extractShortcut": "F6",
                "extractShortcut2": "",
                "textStrengtheningEnabled": true,
                SCHEMA_VERSION_KEY: 1
            }
        });

        // 1. User disables text strengthening via shortcut domain patch
        let persist_disable = ShortcutDomainPersist {
            text_strengthening_enabled: Some(false),
            ..Default::default()
        };
        apply_shortcut_domain_persist(&mut raw, &persist_disable).unwrap();
        assert_eq!(raw["general"]["shortcut2"], "F8");
        assert_eq!(raw["general"]["textStrengtheningEnabled"], false);

        // 2. User updates non-shortcut settings; domain fields are protected
        let incoming_theme = json!({
            "general": {
                "theme": "dark",
                "textStrengtheningEnabled": true, // Stale frontend snapshot tries to overwrite
                "shortcut2": "ShouldNotOverwrite"
            }
        });
        merge_config_preserving_shortcuts(&mut raw, incoming_theme);
        assert_eq!(raw["general"]["shortcut2"], "F8");
        assert_eq!(raw["general"]["textStrengtheningEnabled"], false);
        assert_eq!(raw["general"]["theme"], "dark");

        // 3. Simulated restart: migration runs, shortcut2 is preserved
        let outcome = migrate_shortcuts(&mut raw, true, false);
        assert_eq!(raw["general"]["shortcut2"], "F8");
        assert!(!outcome.changed);

        // 4. User re-enables text strengthening via shortcut domain patch
        let persist_reenable = ShortcutDomainPersist {
            text_strengthening_enabled: Some(true),
            ..Default::default()
        };
        apply_shortcut_domain_persist(&mut raw, &persist_reenable).unwrap();
        assert_eq!(raw["general"]["shortcut2"], "F8");
        assert_eq!(raw["general"]["textStrengtheningEnabled"], true);
    }

    #[test]
    fn migration_with_strengthening_disabled_preserves_stored_secondary_value() {
        // Windows schema < 1 with F4 and secondary Alt+A (would conflict if enabled)
        let mut raw = json!({
            "general": {
                "shortcut": "F4",
                "shortcut2": "Alt+A",
                "extractShortcut": "F6",
                "extractShortcut2": "",
                "textStrengtheningEnabled": false
            }
        });

        let outcome = migrate_shortcuts(&mut raw, true, true);
        // F4 -> AltRight succeeds because inactive shortcut2 is masked from conflict check
        assert_eq!(general_shortcut(&raw), Some("AltRight"));
        assert!(outcome.f4_replaced);
        assert!(!outcome.f4_kept_conflict);
        // Stored shortcut2 remains Alt+A untouched
        assert_eq!(raw["general"]["shortcut2"], "Alt+A");
    }

    #[test]
    fn migration_with_strengthening_disabled_and_secondary_alt_right() {
        // Windows schema < 1 with F4 and secondary AltRight
        let mut raw = json!({
            "general": {
                "shortcut": "F4",
                "shortcut2": "AltRight",
                "extractShortcut": "F6",
                "extractShortcut2": "",
                "textStrengtheningEnabled": false
            }
        });

        let outcome = migrate_shortcuts(&mut raw, true, true);
        assert_eq!(general_shortcut(&raw), Some("AltRight"));
        assert!(outcome.f4_replaced);
        assert!(!outcome.f4_kept_conflict);
        assert_eq!(raw["general"]["shortcut2"], "AltRight");
    }

    #[test]
    fn merge_preserving_shortcuts_protects_templates_and_strengthening() {
        let mut raw = json!({
            "general": {
                "shortcut": "AltRight",
                "shortcutTemplate": "tpl-1",
                "textStrengtheningEnabled": false,
                SCHEMA_VERSION_KEY: 1
            }
        });
        let incoming = json!({
            "general": {
                "shortcut": "F4",
                "shortcutTemplate": "tpl-stale",
                "textStrengtheningEnabled": true,
                "theme": "dark"
            }
        });
        merge_config_preserving_shortcuts(&mut raw, incoming);
        // Protected domain fields are untouched
        assert_eq!(raw["general"]["shortcut"], "AltRight");
        assert_eq!(raw["general"]["shortcutTemplate"], "tpl-1");
        assert_eq!(raw["general"]["textStrengtheningEnabled"], false);
        // Non-shortcut field was updated
        assert_eq!(raw["general"]["theme"], "dark");
    }

    #[test]
    fn stale_full_config_save_cannot_revert_newer_key_template_or_strengthening() {
        let mut raw = json!({
            "general": {
                "shortcut": "F8",
                "shortcut2": "F9",
                "shortcutTemplate": "new-tpl",
                "textStrengtheningEnabled": true,
                "theme": "light",
                SCHEMA_VERSION_KEY: 1
            }
        });

        // A stale frontend snapshot arrives from an older state
        let stale_incoming = json!({
            "general": {
                "shortcut": "AltRight",
                "shortcut2": "F4",
                "shortcutTemplate": "old-tpl",
                "textStrengtheningEnabled": false,
                "theme": "dark" // User only changed theme in this save
            }
        });

        merge_config_preserving_shortcuts(&mut raw, stale_incoming);

        // The shortcut domain keys are strictly preserved
        assert_eq!(raw["general"]["shortcut"], "F8");
        assert_eq!(raw["general"]["shortcut2"], "F9");
        assert_eq!(raw["general"]["shortcutTemplate"], "new-tpl");
        assert_eq!(raw["general"]["textStrengtheningEnabled"], true);

        // Non-domain key is updated
        assert_eq!(raw["general"]["theme"], "dark");
    }
}
