import test from 'node:test'
import assert from 'node:assert'
import {
  computeStrengtheningPatch,
  executeTemplateDeletion,
  mergeConfirmedShortcutDomain,
  planTemplateDeletionReassignment,
} from '../src/core/shortcut-domain.ts'
import type { GeneralConfig, ShortcutUpdateResult } from '../src/core/types.ts'

test('enabling with nonempty bindings emits only strengthening flag', () => {
  const patch = computeStrengtheningPatch(
    true,
    { shortcutTemplate: 'existing-style-1', shortcut2Template: 'existing-style-2' },
    [{ id: 'fallback-1' }, { id: 'fallback-2' }],
  )
  assert.deepStrictEqual(patch, { textStrengtheningEnabled: true })
})

test('enabling with empty bindings emits required fallback fields', () => {
  const patch = computeStrengtheningPatch(
    true,
    { shortcutTemplate: '', shortcut2Template: '' },
    [{ id: 'fallback-1' }, { id: 'fallback-2' }],
  )
  assert.deepStrictEqual(patch, {
    textStrengtheningEnabled: true,
    shortcutTemplate: 'fallback-1',
    shortcut2Template: 'fallback-2',
  })
})

test('disabling strengthening emits only flag', () => {
  const patch = computeStrengtheningPatch(
    false,
    { shortcutTemplate: 'existing-style-1' },
    [{ id: 'fallback-1' }],
  )
  assert.deepStrictEqual(patch, { textStrengtheningEnabled: false })
})

test('confirmed responses update all nine domain fields while preserving pending theme and microphone edits', () => {
  const general: GeneralConfig = {
    theme: 'dark',
    microphone: 'usb-mic-1',
    launchAtLogin: true,
    shortcut: 'old-1',
    shortcut2: 'old-2',
    extractShortcut: 'old-3',
    extractShortcut2: 'old-4',
    shortcutTemplate: 'old-tpl-1',
    shortcut2Template: 'old-tpl-2',
    extractShortcutTemplate: 'old-tpl-3',
    extractShortcut2Template: 'old-tpl-4',
    textStrengtheningEnabled: false,
    maxRecordingSeconds: 60,
    overwriteClipboard: true,
  }

  const confirmed: ShortcutUpdateResult = {
    shortcut: 'AltRight',
    shortcut2: 'F8',
    extractShortcut: 'F6',
    extractShortcut2: 'F7',
    shortcutTemplate: 'new-tpl-1',
    shortcut2Template: 'new-tpl-2',
    extractShortcutTemplate: 'new-tpl-3',
    extractShortcut2Template: 'new-tpl-4',
    textStrengtheningEnabled: true,
    altRightBackend: 'raw_input',
    perField: [],
  }

  const merged = mergeConfirmedShortcutDomain(general, confirmed)

  // All 9 shortcut-domain fields updated
  assert.strictEqual(merged.shortcut, 'AltRight')
  assert.strictEqual(merged.shortcut2, 'F8')
  assert.strictEqual(merged.extractShortcut, 'F6')
  assert.strictEqual(merged.extractShortcut2, 'F7')
  assert.strictEqual(merged.shortcutTemplate, 'new-tpl-1')
  assert.strictEqual(merged.shortcut2Template, 'new-tpl-2')
  assert.strictEqual(merged.extractShortcutTemplate, 'new-tpl-3')
  assert.strictEqual(merged.extractShortcut2Template, 'new-tpl-4')
  assert.strictEqual(merged.textStrengtheningEnabled, true)

  // Pending unrelated edits preserved
  assert.strictEqual(merged.theme, 'dark')
  assert.strictEqual(merged.microphone, 'usb-mic-1')
  assert.strictEqual(merged.launchAtLogin, true)
  assert.strictEqual(merged.maxRecordingSeconds, 60)
})

test('template deletion fails closed on reassignment rejection (real executeTemplateDeletion)', async () => {
  let onSuccessCalled = false
  let errorMessage: string | null = null

  const templates = [
    { id: 'custom-to-delete', name: 'Custom' },
    { id: 'fallback-1', name: 'Fallback' },
  ]
  const general: GeneralConfig = {
    theme: 'light',
    microphone: 'default',
    launchAtLogin: false,
    shortcut: 'AltRight',
    shortcut2: '',
    extractShortcut: 'F6',
    extractShortcut2: '',
    shortcutTemplate: 'custom-to-delete',
    shortcut2Template: '',
    extractShortcutTemplate: '',
    extractShortcut2Template: '',
    maxRecordingSeconds: 60,
    overwriteClipboard: true,
    textStrengtheningEnabled: true,
  }

  const success = await executeTemplateDeletion({
    kind: 'voice',
    currentId: 'custom-to-delete',
    templates,
    general,
    updateShortcutsFn: async () => {
      throw new Error('OS Registration Error')
    },
    onSuccess: () => {
      onSuccessCalled = true
    },
    onError: (msg) => {
      errorMessage = msg
    },
  })

  assert.strictEqual(success, false)
  assert.strictEqual(onSuccessCalled, false, 'onSuccess must NEVER be called on reassignment error')
  assert.ok(errorMessage?.includes('OS Registration Error'))
})

test('template deletion success preserves newer concurrent edits in latest state', async () => {
  const templates = [
    { id: 'custom-to-delete', name: 'Custom' },
    { id: 'fallback-1', name: 'Fallback' },
  ]

  // State when user clicked delete
  const initialGeneral: GeneralConfig = {
    theme: 'light',
    microphone: 'mic-old',
    launchAtLogin: false,
    shortcut: 'AltRight',
    shortcut2: '',
    extractShortcut: 'F6',
    extractShortcut2: '',
    shortcutTemplate: 'custom-to-delete',
    shortcut2Template: 'other',
    extractShortcutTemplate: '',
    extractShortcut2Template: '',
    maxRecordingSeconds: 60,
    overwriteClipboard: true,
    textStrengtheningEnabled: true,
  }

  // Live state updated concurrently while reassignment was in flight
  let liveState = {
    general: {
      ...initialGeneral,
      theme: 'dark', // Concurrently edited!
      microphone: 'mic-new', // Concurrently edited!
    },
    voiceTemplates: { templates },
  }

  const success = await executeTemplateDeletion({
    kind: 'voice',
    currentId: 'custom-to-delete',
    templates,
    general: initialGeneral,
    updateShortcutsFn: async (_patch) => {
      return {
        shortcut: 'AltRight',
        shortcut2: '',
        extractShortcut: 'F6',
        extractShortcut2: '',
        shortcutTemplate: 'fallback-1',
        shortcut2Template: 'other',
        extractShortcutTemplate: '',
        extractShortcut2Template: '',
        textStrengtheningEnabled: true,
        altRightBackend: 'raw_input',
        perField: [],
      }
    },
    onSuccess: ({ deletedId, fallbackId, confirmedDomain }) => {
      // In App.tsx, composition uses the latest liveState!
      let general = liveState.general
      if (confirmedDomain) {
        general = mergeConfirmedShortcutDomain(general, confirmedDomain)
      }
      liveState = {
        ...liveState,
        general,
        voiceTemplates: {
          templates: liveState.voiceTemplates.templates.filter(t => t.id !== deletedId),
        },
      }
      assert.strictEqual(fallbackId, 'fallback-1')
    },
    onError: () => {},
  })

  assert.strictEqual(success, true)
  // Preserved newer concurrent edits!
  assert.strictEqual(liveState.general.theme, 'dark', 'Newer theme edit must be preserved')
  assert.strictEqual(liveState.general.microphone, 'mic-new', 'Newer microphone edit must be preserved')
  // Confirmed fallback template was applied
  assert.strictEqual(liveState.general.shortcutTemplate, 'fallback-1')
  // Template list removed deleted template
  assert.strictEqual(liveState.voiceTemplates.templates.length, 1)
})
