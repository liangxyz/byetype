//! Shortcut domain transaction planning, validation and execution orchestration.
//!
//! Handles candidate planning, conflict validation, registration execution,
//! atomic persistence of shortcut keys, templates, and text-strengthening flag,
//! and complete rollback on failure.

use serde::{Deserialize, Serialize};

use crate::shortcut::model::{
    self, build_conflict_graph, is_alt_right, NormalizedShortcut, ShortcutField, ALT_RIGHT,
};
use crate::shortcut::native::{AltRightBackend, BackendKind};
use crate::config::shortcut_config::ShortcutDomainPersist;

/// Sparse shortcut-domain patch. `None` = do not touch; `Some("")` = clear.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutDomainPatch {
    #[serde(default)]
    pub shortcut: Option<String>,
    #[serde(default)]
    pub shortcut2: Option<String>,
    #[serde(default)]
    pub extract_shortcut: Option<String>,
    #[serde(default)]
    pub extract_shortcut2: Option<String>,
    #[serde(default)]
    pub shortcut_template: Option<String>,
    #[serde(default)]
    pub shortcut2_template: Option<String>,
    #[serde(default)]
    pub extract_shortcut_template: Option<String>,
    #[serde(default)]
    pub extract_shortcut2_template: Option<String>,
    #[serde(default)]
    pub text_strengthening_enabled: Option<bool>,
}

impl ShortcutDomainPatch {
    pub fn is_empty(&self) -> bool {
        self.shortcut.is_none()
            && self.shortcut2.is_none()
            && self.extract_shortcut.is_none()
            && self.extract_shortcut2.is_none()
            && self.shortcut_template.is_none()
            && self.shortcut2_template.is_none()
            && self.extract_shortcut_template.is_none()
            && self.extract_shortcut2_template.is_none()
            && self.text_strengthening_enabled.is_none()
    }
}

pub type ShortcutPatch = ShortcutDomainPatch;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldResult {
    pub field: String,
    pub value: String,
    pub registered: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutUpdateResult {
    pub shortcut: String,
    pub shortcut2: String,
    pub extract_shortcut: String,
    pub extract_shortcut2: String,
    pub alt_right_backend: Option<String>,
    pub per_field: Vec<FieldResult>,
}

#[derive(Debug, Clone)]
pub struct FieldRuntime {
    pub field: ShortcutField,
    pub value: String,
    pub normalized: NormalizedShortcut,
    pub template: String,
    pub registered: bool,
    pub uses_native_alt_right: bool,
    pub skip_reason: Option<String>,
}

impl FieldRuntime {
    pub fn empty(field: ShortcutField) -> Self {
        Self {
            field,
            value: String::new(),
            normalized: NormalizedShortcut::Empty,
            template: String::new(),
            registered: false,
            uses_native_alt_right: false,
            skip_reason: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct RegistrationResult {
    pub registered: [bool; 4],
    pub backend: Option<AltRightBackend>,
    pub backend_kind: Option<BackendKind>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TransactionPlan {
    pub values: [String; 4],
    pub templates: [String; 4],
    pub normalized: [NormalizedShortcut; 4],
    pub target_strengthening: bool,
    pub active: [bool; 4],
    pub fields: [FieldRuntime; 4],
    pub persist: ShortcutDomainPersist,
}

pub fn normalized_from_values(values: &[String; 4]) -> [NormalizedShortcut; 4] {
    std::array::from_fn(|i| model::normalize(&values[i]))
}

pub fn build_fields(
    values: &[String; 4],
    normalized: &[NormalizedShortcut; 4],
    graph: &model::ConflictGraph,
    templates: &[String; 4],
    active: &[bool; 4],
    is_windows: bool,
) -> [FieldRuntime; 4] {
    std::array::from_fn(|index| {
        let field = ShortcutField::ALL[index];
        let mut runtime = FieldRuntime::empty(field);
        runtime.value = values[index].clone();
        runtime.normalized = normalized[index].clone();
        runtime.template = templates[index].clone();

        if !active[index] {
            runtime.registered = false;
            runtime.uses_native_alt_right = false;
            runtime.skip_reason = Some("TEXT_STRENGTHENING_DISABLED: 文本强化已关闭".to_string());
            return runtime;
        }

        match &normalized[index] {
            NormalizedShortcut::Empty => {}
            NormalizedShortcut::Invalid(raw) => {
                runtime.skip_reason = Some(format!("INVALID_SHORTCUT: {}", raw));
            }
            NormalizedShortcut::Valid(canonical) => {
                if graph.skipped[index] {
                    runtime.skip_reason = Some("SHORTCUT_CONFLICT".to_string());
                } else if is_alt_right(canonical) {
                    if is_windows {
                        runtime.registered = true;
                        runtime.uses_native_alt_right = true;
                    } else {
                        runtime.skip_reason =
                            Some("ALT_RIGHT_BACKEND_FAILED: 当前平台不支持右 Alt".to_string());
                    }
                } else {
                    runtime.registered = true;
                }
            }
        }
        runtime
    })
}

pub fn build_update_result(
    fields: &[FieldRuntime; 4],
    normalized: &[NormalizedShortcut; 4],
    backend_kind: Option<BackendKind>,
    persist_keys: &[(ShortcutField, String)],
) -> ShortcutUpdateResult {
    let confirmed = |index: usize| -> String {
        match &normalized[index] {
            NormalizedShortcut::Empty => String::new(),
            NormalizedShortcut::Valid(canonical) => canonical.clone(),
            NormalizedShortcut::Invalid(raw) => raw.clone(),
        }
    };
    let per_field = ShortcutField::ALL
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let persisted = persist_keys
                .iter()
                .find(|(patched, _)| patched == field)
                .map(|(_, value)| value.clone());
            FieldResult {
                field: field.key().to_string(),
                value: persisted.unwrap_or_else(|| confirmed(index)),
                registered: fields[index].registered,
                error: fields[index].skip_reason.clone(),
            }
        })
        .collect();

    ShortcutUpdateResult {
        shortcut: confirmed(0),
        shortcut2: confirmed(1),
        extract_shortcut: confirmed(2),
        extract_shortcut2: confirmed(3),
        alt_right_backend: backend_kind.map(|kind| kind.as_str().to_string()),
        per_field,
    }
}

pub fn plan_shortcut_transaction(
    current_values: &[String; 4],
    current_templates: &[String; 4],
    current_strengthening: bool,
    patch: &ShortcutDomainPatch,
    is_windows: bool,
) -> Result<TransactionPlan, String> {
    let mut values = current_values.clone();
    let mut key_changed = [false; 4];
    let key_entries: [Option<&String>; 4] = [
        patch.shortcut.as_ref(),
        patch.shortcut2.as_ref(),
        patch.extract_shortcut.as_ref(),
        patch.extract_shortcut2.as_ref(),
    ];
    for (i, opt) in key_entries.into_iter().enumerate() {
        if let Some(val) = opt {
            values[i] = val.clone();
            key_changed[i] = true;
        }
    }

    let mut templates = current_templates.clone();
    let mut template_changed = [false; 4];
    let template_entries: [Option<&String>; 4] = [
        patch.shortcut_template.as_ref(),
        patch.shortcut2_template.as_ref(),
        patch.extract_shortcut_template.as_ref(),
        patch.extract_shortcut2_template.as_ref(),
    ];
    for (i, opt) in template_entries.into_iter().enumerate() {
        if let Some(val) = opt {
            templates[i] = val.clone();
            template_changed[i] = true;
        }
    }

    let target_strengthening = patch
        .text_strengthening_enabled
        .unwrap_or(current_strengthening);
    let strengthening_changed = patch.text_strengthening_enabled.is_some()
        && patch.text_strengthening_enabled != Some(current_strengthening);

    let active = [true, target_strengthening, true, true];
    let newly_activated = !current_strengthening && target_strengthening;
    let syntax_check_mask = [
        key_changed[0],
        key_changed[1] || newly_activated,
        key_changed[2],
        key_changed[3],
    ];

    let normalized = normalized_from_values(&values);
    model::validate_candidates_with_active(&normalized, &syntax_check_mask, &active).map_err(
        |error| format!("{}: {}", error.code, error.message),
    )?;

    if !is_windows {
        for index in 0..4 {
            if (key_changed[index] || (index == 1 && newly_activated))
                && normalized[index].canonical() == Some(ALT_RIGHT)
            {
                return Err("ALT_RIGHT_BACKEND_FAILED: 当前平台不支持右 Alt 快捷键".to_string());
            }
        }
    }

    let conflict_candidates = model::effective_conflict_candidates(&normalized, &active);
    let graph = build_conflict_graph(&conflict_candidates);
    let fields = build_fields(&values, &normalized, &graph, &templates, &active, is_windows);

    let mut persist = ShortcutDomainPersist::default();
    for index in 0..4 {
        if key_changed[index] {
            let val_str = match &normalized[index] {
                NormalizedShortcut::Empty => String::new(),
                NormalizedShortcut::Valid(c) => c.clone(),
                NormalizedShortcut::Invalid(r) => r.clone(),
            };
            persist.keys.push((ShortcutField::ALL[index], val_str));
        }
        if template_changed[index] {
            persist.templates.push((ShortcutField::ALL[index], templates[index].clone()));
        }
    }
    if strengthening_changed {
        persist.text_strengthening_enabled = Some(target_strengthening);
    }

    Ok(TransactionPlan {
        values,
        templates,
        normalized,
        target_strengthening,
        active,
        fields,
        persist,
    })
}

pub fn execute_transaction_workflow<R, P, B, S>(
    plan: &TransactionPlan,
    old_fields: &[FieldRuntime; 4],
    mut register_fn: R,
    mut teardown_fn: impl FnMut(),
    mut persist_fn: P,
    mut stop_backend_fn: B,
    mut install_restored_fn: S,
) -> Result<RegistrationResult, String>
where
    R: FnMut(&[FieldRuntime; 4]) -> RegistrationResult,
    P: FnMut(&ShortcutDomainPersist) -> Result<(), String>,
    B: FnMut(&mut Option<AltRightBackend>),
    S: FnMut(RegistrationResult),
{
    teardown_fn();
    let mut result = register_fn(&plan.fields);
    if !result.errors.is_empty() {
        stop_backend_fn(&mut result.backend);
        teardown_fn();
        let mut rollback = register_fn(old_fields);
        if !rollback.errors.is_empty() {
            stop_backend_fn(&mut rollback.backend);
            teardown_fn();
            return Err(format!(
                "SHORTCUT_ROLLBACK_FAILED: 新注册失败: {}; 回滚失败: {}",
                result.errors.join("; "),
                rollback.errors.join("; ")
            ));
        }
        install_restored_fn(rollback);
        return Err(format!(
            "SHORTCUT_REGISTER_FAILED: {}",
            result.errors.join("; ")
        ));
    }

    if !plan.persist.is_empty() {
        if let Err(error) = persist_fn(&plan.persist) {
            stop_backend_fn(&mut result.backend);
            teardown_fn();
            let mut rollback = register_fn(old_fields);
            if !rollback.errors.is_empty() {
                stop_backend_fn(&mut rollback.backend);
                teardown_fn();
                return Err(format!(
                    "SHORTCUT_ROLLBACK_FAILED: 写盘失败后回滚失败: {}; {}",
                    error,
                    rollback.errors.join("; ")
                ));
            }
            install_restored_fn(rollback);
            return Err(error);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use crate::shortcut::native;

    #[test]
    fn plan_transaction_normalizes_backspace_to_empty() {
        let current_values = std::array::from_fn(|_| String::new());
        let current_templates = std::array::from_fn(|_| String::new());
        let patch = ShortcutDomainPatch {
            shortcut: Some("Backspace".to_string()),
            ..Default::default()
        };
        let plan = plan_shortcut_transaction(
            &current_values,
            &current_templates,
            false,
            &patch,
            true,
        )
        .unwrap();
        assert_eq!(
            plan.persist.keys,
            vec![(ShortcutField::Shortcut, String::new())]
        );
    }

    #[test]
    fn plan_transaction_canonicalizes_valid_values() {
        let current_values = std::array::from_fn(|_| String::new());
        let current_templates = std::array::from_fn(|_| String::new());
        let patch = ShortcutDomainPatch {
            shortcut: Some("ctrl+shift+a".to_string()),
            ..Default::default()
        };
        let plan = plan_shortcut_transaction(
            &current_values,
            &current_templates,
            false,
            &patch,
            true,
        )
        .unwrap();
        assert_eq!(
            plan.persist.keys,
            vec![(ShortcutField::Shortcut, "Ctrl+Shift+A".to_string())]
        );
    }

    #[test]
    fn transaction_plan_resolves_conflict_on_simultaneous_reenable() {
        let current_values = [
            "F4".to_string(),
            "F4".to_string(),
            "F6".to_string(),
            String::new(),
        ];
        let current_templates = std::array::from_fn(|_| "tpl".to_string());
        let patch = ShortcutDomainPatch {
            shortcut2: Some("F5".to_string()),
            text_strengthening_enabled: Some(true),
            ..Default::default()
        };
        let plan = plan_shortcut_transaction(
            &current_values,
            &current_templates,
            false,
            &patch,
            true,
        )
        .expect("plan should resolve duplicate conflict");

        assert_eq!(plan.target_strengthening, true);
        assert_eq!(plan.values[1], "F5");
        assert_eq!(plan.persist.keys, vec![(ShortcutField::Shortcut2, "F5".to_string())]);
        assert_eq!(plan.persist.text_strengthening_enabled, Some(true));
    }

    #[test]
    fn transaction_plan_conflicting_patch_on_reenable_fails() {
        let current_values = [
            "F4".to_string(),
            "F8".to_string(),
            "F6".to_string(),
            String::new(),
        ];
        let current_templates = std::array::from_fn(|_| "tpl".to_string());
        let patch = ShortcutDomainPatch {
            shortcut2: Some("F4".to_string()),
            text_strengthening_enabled: Some(true),
            ..Default::default()
        };
        let err = plan_shortcut_transaction(
            &current_values,
            &current_templates,
            false,
            &patch,
            true,
        )
        .unwrap_err();
        assert!(err.contains("SHORTCUT_CONFLICT"));
    }

    #[test]
    fn transaction_plan_newly_activated_invalid_syntax_fails() {
        let current_values = [
            "F4".to_string(),
            "InvalidKeySyntax".to_string(),
            "F6".to_string(),
            String::new(),
        ];
        let current_templates = std::array::from_fn(|_| "tpl".to_string());
        let patch = ShortcutDomainPatch {
            text_strengthening_enabled: Some(true),
            ..Default::default()
        };
        let err = plan_shortcut_transaction(
            &current_values,
            &current_templates,
            false,
            &patch,
            true,
        )
        .unwrap_err();
        assert!(err.contains("INVALID_SHORTCUT"));
    }

    #[test]
    fn transaction_plan_key_only_patch_touches_no_templates_or_flag() {
        let current_values = [
            "F4".to_string(),
            "F5".to_string(),
            "F6".to_string(),
            String::new(),
        ];
        let current_templates = [
            "t1".to_string(),
            "t2".to_string(),
            "t3".to_string(),
            "t4".to_string(),
        ];
        let patch = ShortcutDomainPatch {
            shortcut: Some("F8".to_string()),
            ..Default::default()
        };
        let plan = plan_shortcut_transaction(
            &current_values,
            &current_templates,
            true,
            &patch,
            true,
        )
        .expect("plan should succeed");

        assert_eq!(plan.persist.keys, vec![(ShortcutField::Shortcut, "F8".to_string())]);
        assert!(plan.persist.templates.is_empty());
        assert_eq!(plan.persist.text_strengthening_enabled, None);
    }

    #[test]
    fn transaction_workflow_preserves_restored_backend_lifetime_on_registration_failure() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let drop_count = Arc::new(AtomicUsize::new(0));
        let drop_count_clone = drop_count.clone();

        let mut restored_backend: Option<AltRightBackend> = None;

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            |fields| {
                if fields[0].value == "F8" {
                    RegistrationResult {
                        errors: vec!["OS Hotkey conflict".to_string()],
                        ..Default::default()
                    }
                } else {
                    let dc = drop_count_clone.clone();
                    let backend = native::make_backend(
                        BackendKind::RawInput,
                        Box::new(move || {
                            dc.fetch_add(1, Ordering::SeqCst);
                        }),
                    );
                    RegistrationResult {
                        backend: Some(backend),
                        backend_kind: Some(BackendKind::RawInput),
                        registered: [true, false, false, false],
                        errors: Vec::new(),
                    }
                }
            },
            || {},
            |_| Ok(()),
            |_| {},
            |restored| {
                restored_backend = restored.backend;
            },
        ).unwrap_err();

        assert!(err.contains("SHORTCUT_REGISTER_FAILED"));
        assert_eq!(drop_count.load(Ordering::SeqCst), 0, "Restored backend must remain alive after error return");
        assert!(restored_backend.is_some(), "Restored backend must be adopted");

        drop(restored_backend);
        assert_eq!(drop_count.load(Ordering::SeqCst), 1, "Backend stop hook runs when adopted owner is dropped");
    }

    #[test]
    fn transaction_workflow_preserves_restored_backend_lifetime_on_persistence_failure() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let drop_count = Arc::new(AtomicUsize::new(0));
        let drop_count_clone = drop_count.clone();

        let mut restored_backend: Option<AltRightBackend> = None;

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            |fields| {
                if fields[0].value == "F8" {
                    RegistrationResult::default()
                } else {
                    let dc = drop_count_clone.clone();
                    let backend = native::make_backend(
                        BackendKind::RawInput,
                        Box::new(move || {
                            dc.fetch_add(1, Ordering::SeqCst);
                        }),
                    );
                    RegistrationResult {
                        backend: Some(backend),
                        backend_kind: Some(BackendKind::RawInput),
                        registered: [true, false, false, false],
                        errors: Vec::new(),
                    }
                }
            },
            || {},
            |_| Err("IO write failure".to_string()),
            |_| {},
            |restored| {
                restored_backend = restored.backend;
            },
        ).unwrap_err();

        assert_eq!(err, "IO write failure");
        assert_eq!(drop_count.load(Ordering::SeqCst), 0, "Restored backend must remain alive after persist error");
        assert!(restored_backend.is_some(), "Restored backend must be adopted");

        drop(restored_backend);
        assert_eq!(drop_count.load(Ordering::SeqCst), 1, "Backend stop hook runs when adopted owner is dropped");
    }

    #[test]
    fn transaction_workflow_stops_candidate_backend_on_rollback_failure() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let candidate_drop_count = Arc::new(AtomicUsize::new(0));
        let candidate_drop_clone = candidate_drop_count.clone();

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            |fields| {
                if fields[0].value == "F8" {
                    let dc = candidate_drop_clone.clone();
                    let backend = native::make_backend(
                        BackendKind::RawInput,
                        Box::new(move || {
                            dc.fetch_add(1, Ordering::SeqCst);
                        }),
                    );
                    RegistrationResult {
                        backend: Some(backend),
                        errors: vec!["Partial registration failure".to_string()],
                        ..Default::default()
                    }
                } else {
                    RegistrationResult {
                        errors: vec!["Rollback registration failure".to_string()],
                        ..Default::default()
                    }
                }
            },
            || {},
            |_| Ok(()),
            |backend_opt| {
                if let Some(mut backend) = backend_opt.take() {
                    backend.stop();
                }
            },
            |_| {},
        ).unwrap_err();

        assert!(err.contains("SHORTCUT_ROLLBACK_FAILED"));
        assert_eq!(candidate_drop_count.load(Ordering::SeqCst), 1, "Candidate backend must be stopped on rollback failure");
    }

    #[test]
    fn strengthening_disabled_deactivates_shortcut2() {
        let values = [
            "F4".to_string(),
            "F5".to_string(),
            String::new(),
            String::new(),
        ];
        let normalized = normalized_from_values(&values);
        let active = [true, false, true, true];
        let conflict_candidates = model::effective_conflict_candidates(&normalized, &active);
        let graph = build_conflict_graph(&conflict_candidates);
        let templates = std::array::from_fn(|_| String::new());
        let fields = build_fields(&values, &normalized, &graph, &templates, &active, true);

        // slot 0 should be registered
        assert!(fields[0].registered);

        // slot 1 (shortcut2) should NOT be registered and have skip_reason
        assert!(!fields[1].registered);
        assert!(!fields[1].uses_native_alt_right);
        assert_eq!(
            fields[1].skip_reason.as_deref(),
            Some("TEXT_STRENGTHENING_DISABLED: 文本强化已关闭")
        );

        let result = build_update_result(&fields, &normalized, None, &[]);
        assert!(result.per_field[0].registered);
        assert!(!result.per_field[1].registered);
        assert_eq!(
            result.per_field[1].error.as_deref(),
            Some("TEXT_STRENGTHENING_DISABLED: 文本强化已关闭")
        );
    }

    #[test]
    fn alt_right_in_disabled_slot1_requires_no_native_backend() {
        let values = [
            "F4".to_string(),
            "AltRight".to_string(),
            String::new(),
            String::new(),
        ];
        let normalized = normalized_from_values(&values);
        let active = [true, false, true, true];
        let conflict_candidates = model::effective_conflict_candidates(&normalized, &active);
        let graph = build_conflict_graph(&conflict_candidates);
        let templates = std::array::from_fn(|_| String::new());
        let fields = build_fields(&values, &normalized, &graph, &templates, &active, true);

        assert!(!fields[1].registered);
        assert!(!fields[1].uses_native_alt_right);
        assert!(!fields.iter().any(|f| f.registered && f.uses_native_alt_right));
    }

    #[test]
    fn disabled_slot1_ignores_duplicate_and_prefix_conflicts() {
        let values_dup = [
            "F4".to_string(),
            "F4".to_string(),
            String::new(),
            String::new(),
        ];
        let normalized_dup = normalized_from_values(&values_dup);
        let active = [true, false, true, true];
        let conflict_candidates = model::effective_conflict_candidates(&normalized_dup, &active);
        let graph_dup = build_conflict_graph(&conflict_candidates);
        let templates = std::array::from_fn(|_| String::new());
        let fields_dup = build_fields(&values_dup, &normalized_dup, &graph_dup, &templates, &active, true);
        assert!(fields_dup[0].registered);
        assert!(!fields_dup[1].registered);

        let values_prefix = [
            "AltRight".to_string(),
            "Alt+A".to_string(),
            String::new(),
            String::new(),
        ];
        let normalized_prefix = normalized_from_values(&values_prefix);
        let conflict_prefix = model::effective_conflict_candidates(&normalized_prefix, &active);
        let graph_prefix = build_conflict_graph(&conflict_prefix);
        let fields_prefix = build_fields(&values_prefix, &normalized_prefix, &graph_prefix, &templates, &active, true);
        assert!(fields_prefix[0].registered);
        assert!(!fields_prefix[1].registered);
    }
}
