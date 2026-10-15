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
