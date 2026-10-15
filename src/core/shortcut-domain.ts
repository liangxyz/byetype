import type { GeneralConfig, ShortcutPatch, ShortcutUpdateResult } from './types'

/**
 * Merge a backend-confirmed shortcut domain update into the current GeneralConfig,
 * preserving pending unrelated fields (theme, microphone, launchAtLogin, etc.).
 */
export function mergeConfirmedShortcutDomain(
  general: GeneralConfig,
  result: ShortcutUpdateResult,
): GeneralConfig {
  return {
    ...general,
    shortcut: result.shortcut,
    shortcut2: result.shortcut2,
    extractShortcut: result.extractShortcut,
    extractShortcut2: result.extractShortcut2,
    shortcutTemplate: result.shortcutTemplate,
    shortcut2Template: result.shortcut2Template,
    extractShortcutTemplate: result.extractShortcutTemplate,
    extractShortcut2Template: result.extractShortcut2Template,
    textStrengtheningEnabled: result.textStrengtheningEnabled,
  }
}

/**
 * Compute the sparse patch for toggling the text-strengthening switch.
 * Fallback templates are only emitted if the current binding is actually empty.
 */
export function computeStrengtheningPatch(
  enabled: boolean,
  current: { shortcutTemplate?: string; shortcut2Template?: string },
  fallbacks: { id: string }[],
): ShortcutPatch {
  const patch: ShortcutPatch = { textStrengtheningEnabled: enabled }
  if (enabled) {
    if (!current.shortcutTemplate) {
      patch.shortcutTemplate = fallbacks[0]?.id || ''
    }
    if (!current.shortcut2Template) {
      patch.shortcut2Template = fallbacks[1]?.id || fallbacks[0]?.id || ''
    }
  }
  return patch
}

/**
 * Plan the sparse patch for reassigning shortcut bindings when deleting a template.
 */
export function planTemplateDeletionReassignment(
  kind: 'voice' | 'image',
  currentId: string,
  general: GeneralConfig,
  remainingTemplates: { id: string }[],
): { patch: ShortcutPatch; fallbackId: string } {
  const fallbackId = remainingTemplates[0]?.id ?? ''
  const patch: ShortcutPatch = {}
  if (kind === 'voice') {
    if (general.shortcutTemplate === currentId) patch.shortcutTemplate = fallbackId
    if (general.shortcut2Template === currentId) patch.shortcut2Template = fallbackId
  } else {
    if (general.extractShortcutTemplate === currentId) patch.extractShortcutTemplate = fallbackId
    if (general.extractShortcut2Template === currentId) patch.extractShortcut2Template = fallbackId
  }
  return { patch, fallbackId }
}

/**
 * Execute template deletion sequencing:
 * 1. Plans any needed binding reassignment;
 * 2. If needed, awaits backend reassignment;
 * 3. On failure: fails closed (calls onError and stops without deleting);
 * 4. On success: calls onSuccess to let the caller update state and remove template.
 */
export async function executeTemplateDeletion({
  kind,
  currentId,
  templates,
  general,
  updateShortcutsFn,
  onSuccess,
  onError,
}: {
  kind: 'voice' | 'image'
  currentId: string
  templates: { id: string; name: string }[]
  general: GeneralConfig
  updateShortcutsFn: (patch: ShortcutPatch) => Promise<ShortcutUpdateResult>
  onSuccess: (data: {
    kind: 'voice' | 'image'
    deletedId: string
    fallbackId: string
    confirmedDomain?: ShortcutUpdateResult
  }) => void
  onError: (error: string) => void
}): Promise<boolean> {
  const remaining = templates.filter(t => t.id !== currentId)
  const { patch, fallbackId } = planTemplateDeletionReassignment(kind, currentId, general, remaining)

  let confirmedDomain: ShortcutUpdateResult | undefined
  if (Object.keys(patch).length > 0) {
    try {
      confirmedDomain = await updateShortcutsFn(patch)
    } catch (e: any) {
      const msg = typeof e === 'string' ? e : e?.message || '更新快捷键绑定失败'
      onError(msg)
      return false
    }
  }

  onSuccess({
    kind,
    deletedId: currentId,
    fallbackId,
    confirmedDomain,
  })
  return true
}
