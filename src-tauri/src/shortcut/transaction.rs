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
    pub shortcut_template: String,
    pub shortcut2_template: String,
    pub extract_shortcut_template: String,
    pub extract_shortcut2_template: String,
    pub text_strengthening_enabled: bool,
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
    pub registered_keys: Vec<String>,
    pub backend: Option<AltRightBackend>,
    pub backend_kind: Option<BackendKind>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TransactionPlan {
    pub templates: [String; 4],
    pub normalized: [NormalizedShortcut; 4],
    pub target_strengthening: bool,
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
    templates: &[String; 4],
    text_strengthening_enabled: bool,
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
        shortcut_template: templates[0].clone(),
        shortcut2_template: templates[1].clone(),
        extract_shortcut_template: templates[2].clone(),
        extract_shortcut2_template: templates[3].clone(),
        text_strengthening_enabled,
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
        templates,
        normalized,
        target_strengthening,
        fields,
        persist,
    })
}

pub fn perform_targeted_cleanup<F>(
    keys_to_clean: &[String],
    owned_ledger: &mut std::collections::HashSet<String>,
    mut unregister_key_fn: F,
) -> Result<(), String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    // Filter to keys currently pending in the ledger.
    // Retrying already-removed keys causes Windows UnregisterHotKey to fail with ERROR_HOTKEY_NOT_REGISTERED.
    let pending_keys: Vec<String> = keys_to_clean
        .iter()
        .filter(|k| owned_ledger.contains(*k))
        .cloned()
        .collect();

    let mut errors = Vec::new();
    for key in &pending_keys {
        match unregister_key_fn(key) {
            Ok(()) => {
                owned_ledger.remove(key);
            }
            Err(e) => {
                owned_ledger.insert(key.clone());
                errors.push(format!("{key}: {e}"));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("UNREGISTER_FAILED: 无法注销快捷键: {}", errors.join("; ")))
    }
}

pub fn execute_transaction_workflow<R, P, B, S, T>(
    plan: &TransactionPlan,
    old_fields: &[FieldRuntime; 4],
    old_registered_keys: &[String],
    mut register_fn: R,
    mut teardown_keys_fn: T,
    mut persist_fn: P,
    mut stop_backend_fn: B,
    mut install_restored_fn: S,
) -> Result<RegistrationResult, String>
where
    R: FnMut(&[FieldRuntime; 4]) -> RegistrationResult,
    P: FnMut(&ShortcutDomainPersist) -> Result<(), String>,
    B: FnMut(&mut Option<AltRightBackend>),
    S: FnMut(RegistrationResult),
    T: FnMut(&[String]) -> Result<(), String>,
{
    teardown_keys_fn(old_registered_keys)?;
    let mut result = register_fn(&plan.fields);
    if !result.errors.is_empty() {
        stop_backend_fn(&mut result.backend);
        if let Err(first_cleanup_err) = teardown_keys_fn(&result.registered_keys) {
            // Targeted teardown of candidate keys failed. Retry once:
            let retry_err = teardown_keys_fn(&result.registered_keys).err();
            let mut errs = result.errors;
            errs.push(first_cleanup_err);
            if let Some(re) = retry_err {
                errs.push(re);
            }
            return Err(format!(
                "SHORTCUT_ROLLBACK_FAILED: 新注册失败且注销清理失败（放弃恢复注册以防状态污染）: {}",
                errs.join("; ")
            ));
        }
        let mut rollback = register_fn(old_fields);
        if !rollback.errors.is_empty() {
            stop_backend_fn(&mut rollback.backend);
            let final_cleanup = teardown_keys_fn(&rollback.registered_keys).err();
            let mut msg = format!(
                "SHORTCUT_ROLLBACK_FAILED: 新注册失败: {}; 回滚失败: {}",
                result.errors.join("; "),
                rollback.errors.join("; ")
            );
            if let Some(fc) = final_cleanup {
                msg.push_str(&format!("; 最终清理失败: {fc}"));
            }
            return Err(msg);
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
            if let Err(first_cleanup_err) = teardown_keys_fn(&result.registered_keys) {
                let retry_err = teardown_keys_fn(&result.registered_keys).err();
                let mut msg = format!(
                    "SHORTCUT_ROLLBACK_FAILED: 写盘失败后注销清理失败: {error}; {first_cleanup_err}"
                );
                if let Some(re) = retry_err {
                    msg.push_str(&format!("; {re}"));
                }
                return Err(msg);
            }
            let mut rollback = register_fn(old_fields);
            if !rollback.errors.is_empty() {
                stop_backend_fn(&mut rollback.backend);
                let final_cleanup = teardown_keys_fn(&rollback.registered_keys).err();
                let mut msg = format!(
                    "SHORTCUT_ROLLBACK_FAILED: 写盘失败后回滚失败: {}; {}",
                    error,
                    rollback.errors.join("; ")
                );
                if let Some(fc) = final_cleanup {
                    msg.push_str(&format!("; 最终清理失败: {fc}"));
                }
                return Err(msg);
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
    use crate::config::shortcut_config;
    use std::sync::Mutex;
    use std::collections::HashSet;

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
        assert_eq!(plan.fields[1].value, "F5");
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
            &[],
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
                        registered_keys: Vec::new(),
                        errors: Vec::new(),
                    }
                }
            },
            |_keys| Ok(()),
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
            &[],
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
                        registered_keys: Vec::new(),
                        errors: Vec::new(),
                    }
                }
            },
            |_keys| Ok(()),
            |_persist| {
                Err("IO write failure".to_string())
            },
            |_| {},
            |restored| {
                restored_backend = restored.backend;
            },
        ).unwrap_err();

        assert_eq!(err, "IO write failure");
        assert_eq!(drop_count.load(Ordering::SeqCst), 0, "Restored backend must remain alive after persist error");
        assert!(restored_backend.is_some());

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
            &[],
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
            |_keys| Ok(()),
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

        assert!(fields[0].registered);
        assert!(!fields[1].registered);
        assert!(!fields[1].uses_native_alt_right);
        assert_eq!(
            fields[1].skip_reason.as_deref(),
            Some("TEXT_STRENGTHENING_DISABLED: 文本强化已关闭")
        );

        let result = build_update_result(&fields, &normalized, &templates, false, None, &[]);
        assert!(result.per_field[0].registered);
        assert!(!result.per_field[1].registered);
        assert_eq!(result.shortcut_template, "");
        assert_eq!(result.text_strengthening_enabled, false);
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

    #[test]
    fn observable_state_successful_transaction_persists_all_fields_together() {
        let mut raw_disk = serde_json::json!({
            "general": {
                "shortcut": "F4",
                "shortcut2": "F5",
                "extractShortcut": "F6",
                "extractShortcut2": "",
                "shortcutTemplate": "t1",
                "shortcut2Template": "t2",
                "extractShortcutTemplate": "t3",
                "extractShortcut2Template": "t4",
                "textStrengtheningEnabled": false,
                "theme": "dark"
            }
        });

        let patch = ShortcutDomainPatch {
            shortcut: Some("F8".to_string()),
            shortcut_template: Some("new-tpl".to_string()),
            text_strengthening_enabled: Some(true),
            ..Default::default()
        };

        let current_values = ["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()];
        let current_templates = ["t1".to_string(), "t2".to_string(), "t3".to_string(), "t4".to_string()];
        let plan = plan_shortcut_transaction(&current_values, &current_templates, false, &patch, true).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let mut registered_map = [false; 4];
        let result = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |fields| {
                for i in 0..4 {
                    registered_map[i] = fields[i].registered;
                }
                RegistrationResult {
                    registered: registered_map,
                    ..Default::default()
                }
            },
            |_keys| Ok(()),
            |persist| {
                shortcut_config::apply_shortcut_domain_persist(&mut raw_disk, persist)
            },
            |_| {},
            |_| {},
        ).unwrap();

        assert!(result.registered[0], "Slot 0 must be registered");
        assert!(result.registered[1], "Slot 1 must now be registered because strengthening is true");

        let update_res = build_update_result(
            &plan.fields,
            &plan.normalized,
            &plan.templates,
            plan.target_strengthening,
            None,
            &plan.persist.keys,
        );
        assert_eq!(update_res.shortcut, "F8");
        assert_eq!(update_res.shortcut_template, "new-tpl");
        assert_eq!(update_res.text_strengthening_enabled, true);

        assert_eq!(raw_disk["general"]["shortcut"], "F8");
        assert_eq!(raw_disk["general"]["shortcutTemplate"], "new-tpl");
        assert_eq!(raw_disk["general"]["textStrengtheningEnabled"], true);
        assert_eq!(raw_disk["general"]["theme"], "dark");
    }

    #[test]
    fn observable_state_registration_failure_leaves_disk_unchanged_and_restores_old_mapping() {
        let mut raw_disk = serde_json::json!({
            "general": {
                "shortcut": "F4",
                "shortcut2": "F5",
                "textStrengtheningEnabled": false
            }
        });

        let patch = ShortcutDomainPatch {
            shortcut: Some("F8".to_string()),
            text_strengthening_enabled: Some(true),
            ..Default::default()
        };

        let current_values = ["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()];
        let current_templates = std::array::from_fn(|_| "".to_string());
        let plan = plan_shortcut_transaction(&current_values, &current_templates, false, &patch, true).unwrap();

        let mut old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));
        old_fields[0].registered = true;
        old_fields[0].value = "F4".to_string();

        let mut current_registered_map = [true, false, false, false];
        let mut restored_result: Option<RegistrationResult> = None;

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &["F4".to_string()],
            |fields| {
                if fields[0].value == "F8" {
                    RegistrationResult {
                        errors: vec!["OS conflict on F8".to_string()],
                        ..Default::default()
                    }
                } else {
                    current_registered_map = [fields[0].registered, fields[1].registered, fields[2].registered, fields[3].registered];
                    RegistrationResult {
                        registered: current_registered_map,
                        ..Default::default()
                    }
                }
            },
            |_keys| Ok(()),
            |persist| shortcut_config::apply_shortcut_domain_persist(&mut raw_disk, persist),
            |_| {},
            |restored| {
                restored_result = Some(restored);
            },
        ).unwrap_err();

        assert!(err.contains("SHORTCUT_REGISTER_FAILED"));
        assert_eq!(raw_disk["general"]["shortcut"], "F4");
        assert_eq!(raw_disk["general"]["textStrengtheningEnabled"], false);
        assert_eq!(current_registered_map, [true, false, false, false]);
        assert!(restored_result.is_some());
    }

    #[test]
    fn observable_state_persistence_failure_restores_old_mapping_and_live_backend() {
        let raw_disk = serde_json::json!({ "general": { "shortcut": "AltRight" } });
        let drop_count = Arc::new(AtomicUsize::new(0));
        let drop_count_clone = drop_count.clone();

        let patch = ShortcutDomainPatch {
            shortcut: Some("F8".to_string()),
            ..Default::default()
        };

        let current_values = ["AltRight".to_string(), "".to_string(), "".to_string(), "".to_string()];
        let current_templates = std::array::from_fn(|_| "".to_string());
        let plan = plan_shortcut_transaction(&current_values, &current_templates, false, &patch, true).unwrap();

        let mut old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));
        old_fields[0].registered = true;
        old_fields[0].uses_native_alt_right = true;

        let mut restored_backend: Option<AltRightBackend> = None;

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |fields| {
                if fields[0].value == "F8" {
                    RegistrationResult {
                        registered: [true, false, false, false],
                        ..Default::default()
                    }
                } else {
                    let dc = drop_count_clone.clone();
                    let backend = native::make_backend(BackendKind::RawInput, Box::new(move || {
                        dc.fetch_add(1, Ordering::SeqCst);
                    }));
                    RegistrationResult {
                        backend: Some(backend),
                        backend_kind: Some(BackendKind::RawInput),
                        registered: [true, false, false, false],
                        registered_keys: Vec::new(),
                        errors: Vec::new(),
                    }
                }
            },
            |_keys| Ok(()),
            |_persist| {
                Err("Simulated disk error".to_string())
            },
            |_| {},
            |restored| {
                restored_backend = restored.backend;
            },
        ).unwrap_err();

        assert_eq!(err, "Simulated disk error");
        assert_eq!(raw_disk["general"]["shortcut"], "AltRight");
        assert_eq!(drop_count.load(Ordering::SeqCst), 0, "Restored backend must NOT be dropped on persist error return");
        assert!(restored_backend.is_some());

        drop(restored_backend);
        assert_eq!(drop_count.load(Ordering::SeqCst), 1, "Backend dropped only after adopted owner drops");
    }

    #[test]
    fn observable_state_teardown_failure_fails_transaction() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "".to_string(), "".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            false,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |_| RegistrationResult::default(),
            |_keys| Err("UNREGISTER_FAILED: OS unregister error".to_string()),
            |_| Ok(()),
            |_| {},
            |_| {},
        ).unwrap_err();

        assert!(err.contains("UNREGISTER_FAILED"));
    }

    #[test]
    fn transaction_workflow_stops_partially_restored_backend_on_rollback_failure() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let restored_drop_count = Arc::new(AtomicUsize::new(0));
        let restored_drop_clone = restored_drop_count.clone();

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |fields| {
                if fields[0].value == "F8" {
                    RegistrationResult {
                        errors: vec!["Initial failure".to_string()],
                        ..Default::default()
                    }
                } else {
                    let dc = restored_drop_clone.clone();
                    let backend = native::make_backend(
                        BackendKind::RawInput,
                        Box::new(move || {
                            dc.fetch_add(1, Ordering::SeqCst);
                        }),
                    );
                    RegistrationResult {
                        backend: Some(backend),
                        errors: vec!["Partial rollback failure".to_string()],
                        ..Default::default()
                    }
                }
            },
            |_keys| Ok(()),
            |_| Ok(()),
            |backend_opt| {
                if let Some(mut backend) = backend_opt.take() {
                    backend.stop();
                }
            },
            |_| {},
        ).unwrap_err();

        assert!(err.contains("SHORTCUT_ROLLBACK_FAILED"));
        assert_eq!(restored_drop_count.load(Ordering::SeqCst), 1, "Partially restored backend must be stopped on rollback failure");
    }

    #[test]
    fn transaction_workflow_teardown_failure_prevents_subsequent_registration() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let mut teardown_calls = 0;
        let mut rollback_registered = false;
        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |fields| {
                if fields[0].value != "F8" {
                    rollback_registered = true;
                }
                RegistrationResult {
                    errors: vec!["Candidate failed".to_string()],
                    ..Default::default()
                }
            },
            |_keys| {
                teardown_calls += 1;
                if teardown_calls > 1 {
                    Err("UNREGISTER_FAILED: Failed to unregister partial candidate".to_string())
                } else {
                    Ok(())
                }
            },
            |_| Ok(()),
            |_| {},
            |_| {},
        ).unwrap_err();

        assert!(err.contains("SHORTCUT_ROLLBACK_FAILED"));
        assert!(err.contains("UNREGISTER_FAILED"));
        assert!(!rollback_registered, "Restoration must never be attempted if unregistering candidate failed");
    }

    #[test]
    fn transaction_workflow_cleanup_retry_clears_partial_registrations_when_successful() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let mut teardown_calls = 0;
        let mut partial_map_cleared = false;

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |_| {
                RegistrationResult {
                    errors: vec!["Candidate registration error".to_string()],
                    ..Default::default()
                }
            },
            |_keys| {
                teardown_calls += 1;
                if teardown_calls == 2 {
                    Err("First teardown failed".to_string())
                } else if teardown_calls == 3 {
                    partial_map_cleared = true;
                    Ok(())
                } else {
                    Ok(())
                }
            },
            |_| Ok(()),
            |_| {},
            |_| {},
        ).unwrap_err();

        assert!(err.contains("SHORTCUT_ROLLBACK_FAILED"));
        assert!(partial_map_cleared, "Retry teardown must clear partial registrations");
        assert_eq!(teardown_calls, 3);
    }

    #[test]
    fn transaction_workflow_repeated_cleanup_failure_is_reported() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let mut teardown_calls = 0;

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |_| {
                RegistrationResult {
                    errors: vec!["Candidate error".to_string()],
                    ..Default::default()
                }
            },
            |_keys| {
                teardown_calls += 1;
                if teardown_calls > 1 {
                    Err(format!("Cleanup error #{teardown_calls}"))
                } else {
                    Ok(())
                }
            },
            |_| Ok(()),
            |_| {},
            |_| {},
        ).unwrap_err();

        assert!(err.contains("SHORTCUT_ROLLBACK_FAILED"));
        assert!(err.contains("Cleanup error #2"));
        assert!(err.contains("Cleanup error #3"));
    }

    #[test]
    fn transaction_workflow_stops_partially_restored_backend_and_cleans_map() {
        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let restored_drop_count = Arc::new(AtomicUsize::new(0));
        let restored_drop_clone = restored_drop_count.clone();
        let final_cleanup_called = Arc::new(AtomicUsize::new(0));
        let final_cleanup_clone = final_cleanup_called.clone();
        let final_registered_map = Arc::new(Mutex::new([false; 4]));
        let final_map_reg = final_registered_map.clone();
        let final_map_clean = final_registered_map.clone();

        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |fields| {
                if fields[0].value == "F8" {
                    RegistrationResult {
                        errors: vec!["Initial failure".to_string()],
                        ..Default::default()
                    }
                } else {
                    let dc = restored_drop_clone.clone();
                    let backend = native::make_backend(
                        BackendKind::RawInput,
                        Box::new(move || {
                            dc.fetch_add(1, Ordering::SeqCst);
                        }),
                    );
                    final_map_reg.lock().unwrap()[0] = true;
                    RegistrationResult {
                        backend: Some(backend),
                        errors: vec!["Partial rollback failure".to_string()],
                        ..Default::default()
                    }
                }
            },
            |_keys| {
                final_cleanup_clone.fetch_add(1, Ordering::SeqCst);
                *final_map_clean.lock().unwrap() = [false; 4];
                Ok(())
            },
            |_| Ok(()),
            |backend_opt| {
                if let Some(mut backend) = backend_opt.take() {
                    backend.stop();
                }
            },
            |_| {},
        ).unwrap_err();

        assert!(err.contains("SHORTCUT_ROLLBACK_FAILED"));
        assert_eq!(restored_drop_count.load(Ordering::SeqCst), 1, "Partially restored backend must be stopped");
        assert!(final_cleanup_called.load(Ordering::SeqCst) > 0, "Final cleanup teardown must be called");
        assert_eq!(*final_registered_map.lock().unwrap(), [false; 4], "Registration map must be cleared on rollback failure");
    }

    #[test]
    fn adapter_fake_reproduces_plugin_mem_take_drain_and_targeted_cleanup_detects_it() {
        // Models tauri-plugin-global-shortcut where unregister_all() drains its map
        struct FakePlugin {
            registered: HashSet<String>,
            os_fail_keys: HashSet<String>,
        }

        impl FakePlugin {
            fn unregister_targeted(&mut self, key: &str) -> Result<(), String> {
                if self.os_fail_keys.contains(key) {
                    Err(format!("OS failed to unregister {key}"))
                } else {
                    self.registered.remove(key);
                    Ok(())
                }
            }

            fn unregister_all_draining(&mut self) -> Result<(), String> {
                let map = std::mem::take(&mut self.registered);
                for key in map {
                    if self.os_fail_keys.contains(&key) {
                        return Err(format!("OS failed to unregister {key}"));
                    }
                }
                Ok(())
            }
        }

        let plugin = Arc::new(Mutex::new(FakePlugin {
            registered: ["F8".to_string()].into_iter().collect(),
            os_fail_keys: ["F8".to_string()].into_iter().collect(),
        }));

        let plan = plan_shortcut_transaction(
            &["F4".to_string(), "F5".to_string(), "F6".to_string(), "".to_string()],
            &std::array::from_fn(|_| "".to_string()),
            true,
            &ShortcutDomainPatch { shortcut: Some("F8".to_string()), ..Default::default() },
            true,
        ).unwrap();
        let old_fields = std::array::from_fn(|i| FieldRuntime::empty(ShortcutField::ALL[i]));

        let p_clean = plugin.clone();
        let err = execute_transaction_workflow(
            &plan,
            &old_fields,
            &[],
            |fields| {
                if fields[0].value == "F8" {
                    RegistrationResult {
                        registered: [true, false, false, false],
                        registered_keys: vec!["F8".to_string()],
                        errors: vec!["Trigger rollback".to_string()],
                        ..Default::default()
                    }
                } else {
                    RegistrationResult::default()
                }
            },
            |keys| {
                let mut p = p_clean.lock().unwrap();
                let mut errs = Vec::new();
                for k in keys {
                    if let Err(e) = p.unregister_targeted(k) {
                        errs.push(e);
                    }
                }
                // Call draining unregister_all
                let _ = p.unregister_all_draining();
                if errs.is_empty() {
                    Ok(())
                } else {
                    Err(errs.join("; "))
                }
            },
            |_| Ok(()),
            |_| {},
            |_| {},
        ).unwrap_err();

        // Targeted check detected the OS unregistration failure despite unregister_all having drained
        assert!(err.contains("SHORTCUT_ROLLBACK_FAILED"));
        assert!(err.contains("OS failed to unregister F8"));
    }

    #[test]
    fn targeted_cleanup_preserves_os_ledger_across_plugin_map_drain_and_retries() {
        // Three-layer fake:
        // 1. plugin_bookkeeping: what plugin thinks is registered (drained on unregister_all)
        // 2. os_registrations: what OS actually holds
        // 3. fail_keys: keys for which OS unregistration fails
        struct ThreeLayerPluginFake {
            plugin_bookkeeping: HashSet<String>,
            os_registrations: HashSet<String>,
            fail_keys: HashSet<String>,
        }

        impl ThreeLayerPluginFake {
            fn unregister_key(&mut self, key: &str) -> Result<(), String> {
                // Real Windows UnregisterHotKey: returns 0 if hotkey is not currently registered in OS
                if !self.os_registrations.contains(key) {
                    return Err(format!("Windows ERROR_HOTKEY_NOT_REGISTERED: {key}"));
                }
                if self.fail_keys.contains(key) {
                    Err(format!("OS failure unregistering {key}"))
                } else {
                    self.plugin_bookkeeping.remove(key);
                    self.os_registrations.remove(key);
                    Ok(())
                }
            }

            fn simulate_unregister_all_draining_bug(&mut self) -> Result<(), String> {
                let drained = std::mem::take(&mut self.plugin_bookkeeping);
                for key in drained {
                    if self.fail_keys.contains(&key) {
                        return Err(format!("OS failed: {key}"));
                    } else {
                        self.os_registrations.remove(&key);
                    }
                }
                Ok(())
            }
        }

        let mut fake = ThreeLayerPluginFake {
            plugin_bookkeeping: ["F8".to_string(), "F9".to_string()].into_iter().collect(),
            os_registrations: ["F8".to_string(), "F9".to_string()].into_iter().collect(),
            fail_keys: ["F8".to_string()].into_iter().collect(), // F8 will fail unregistering
        };

        let mut owned_ledger: HashSet<String> = ["F8".to_string(), "F9".to_string()].into_iter().collect();

        // 1. Initial targeted cleanup: F9 succeeds, F8 fails
        let keys_to_clean = vec!["F8".to_string(), "F9".to_string()];
        let err = perform_targeted_cleanup(
            &keys_to_clean,
            &mut owned_ledger,
            |k| fake.unregister_key(k),
        ).unwrap_err();

        assert!(err.contains("F8"));
        // F9 was cleared from OS and ledger
        assert!(!fake.os_registrations.contains("F9"));
        assert!(!owned_ledger.contains("F9"));
        // F8 failed: retained in OS and retained in owned_ledger!
        assert!(fake.os_registrations.contains("F8"));
        assert!(owned_ledger.contains("F8"), "Failed key must be retained in ledger!");

        // 2. Simulate plugin draining bug happening in background
        let _ = fake.simulate_unregister_all_draining_bug();
        assert!(fake.plugin_bookkeeping.is_empty(), "Plugin bookkeeping was emptied by bug");
        assert!(fake.os_registrations.contains("F8"), "OS still holds F8!");

        // 3. Ledger still remembers F8! Retry targeted cleanup while failure persists:
        let retry_keys: Vec<String> = owned_ledger.iter().cloned().collect();
        assert_eq!(retry_keys, vec!["F8".to_string()]);
        let err_retry = perform_targeted_cleanup(
            &retry_keys,
            &mut owned_ledger,
            |k| fake.unregister_key(k),
        ).unwrap_err();
        assert!(err_retry.contains("F8"));
        assert!(owned_ledger.contains("F8"), "Still retained across repeated failure");

        // 4. Resolve the OS failure (e.g. key lock released) and retry:
        fake.fail_keys.clear();
        let retry_success_keys: Vec<String> = owned_ledger.iter().cloned().collect();
        let ok = perform_targeted_cleanup(
            &retry_success_keys,
            &mut owned_ledger,
            |k| fake.unregister_key(k),
        );
        assert!(ok.is_ok());
        // Now F8 is completely cleared from OS and ledger!
        assert!(!fake.os_registrations.contains("F8"));
        assert!(owned_ledger.is_empty());
    }

    #[test]
    fn registration_immediately_records_ownership_and_cleans_up_on_subsequent_failure() {
        struct MockOs {
            os_registrations: HashSet<String>,
        }

        let mut os = MockOs {
            os_registrations: HashSet::new(),
        };

        let mut owned_ledger: HashSet<String> = HashSet::new();

        // Simulate perform_registration: field 0 ("F8") succeeds and records ownership immediately
        let key = "F8".to_string();
        os.os_registrations.insert(key.clone());
        owned_ledger.insert(key.clone());

        // Field 1 fails registration, triggering rollback
        assert!(owned_ledger.contains("F8"));
        assert!(os.os_registrations.contains("F8"));

        // Cleanup receives candidate registered_keys [&"F8"]
        let res = perform_targeted_cleanup(
            &["F8".to_string()],
            &mut owned_ledger,
            |k| {
                if !os.os_registrations.contains(k) {
                    return Err(format!("Windows ERROR_HOTKEY_NOT_REGISTERED: {k}"));
                }
                os.os_registrations.remove(k);
                Ok(())
            },
        );

        assert!(res.is_ok());
        assert!(!os.os_registrations.contains("F8"), "OS registration must be removed");
        assert!(!owned_ledger.contains("F8"), "Ledger must be cleared");
    }

    #[test]
    fn retry_with_original_two_key_list_never_resurrects_already_removed_key() {
        struct MockOs {
            os_registrations: HashSet<String>,
            fail_unregister_keys: HashSet<String>,
            unregistration_calls: Vec<String>,
        }

        let mut os = MockOs {
            os_registrations: ["KeyA".to_string(), "KeyB".to_string()].into_iter().collect(),
            fail_unregister_keys: ["KeyB".to_string()].into_iter().collect(), // KeyB fails initially
            unregistration_calls: Vec::new(),
        };

        let mut owned_ledger: HashSet<String> = ["KeyA".to_string(), "KeyB".to_string()].into_iter().collect();

        // First attempt with original two-key list
        let original_list = vec!["KeyA".to_string(), "KeyB".to_string()];
        let err = perform_targeted_cleanup(
            &original_list,
            &mut owned_ledger,
            |k| {
                os.unregistration_calls.push(k.to_string());
                if !os.os_registrations.contains(k) {
                    return Err(format!("Windows ERROR_HOTKEY_NOT_REGISTERED: {k}"));
                }
                if os.fail_unregister_keys.contains(k) {
                    return Err(format!("OS failure on {k}"));
                }
                os.os_registrations.remove(k);
                Ok(())
            },
        ).unwrap_err();

        assert!(err.contains("KeyB"));
        assert!(!os.os_registrations.contains("KeyA"), "KeyA was successfully removed from OS");
        assert!(!owned_ledger.contains("KeyA"), "KeyA was removed from ledger");
        assert!(owned_ledger.contains("KeyB"), "KeyB failed and remains in ledger");

        // Second attempt: passing the original two-key list again!
        // KeyA must NOT be resent to the adapter (which would return ERROR_HOTKEY_NOT_REGISTERED)!
        os.unregistration_calls.clear();
        os.fail_unregister_keys.clear(); // KeyB will now succeed

        let ok = perform_targeted_cleanup(
            &original_list,
            &mut owned_ledger,
            |k| {
                os.unregistration_calls.push(k.to_string());
                if !os.os_registrations.contains(k) {
                    return Err(format!("Windows ERROR_HOTKEY_NOT_REGISTERED: {k}"));
                }
                os.os_registrations.remove(k);
                Ok(())
            },
        );

        assert!(ok.is_ok());
        // Verify KeyA was NOT called again! Only KeyB was targeted!
        assert_eq!(os.unregistration_calls, vec!["KeyB".to_string()]);
        assert!(!os.os_registrations.contains("KeyB"));
        assert!(owned_ledger.is_empty());
    }
}
