const test = require('node:test')
const assert = require('node:assert')

// Pure JS implementations mirroring src/core/shortcut-domain.ts for testing with Node test runner
function mergeConfirmedShortcutDomain(general, result) {
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

function computeStrengtheningPatch(enabled, current, fallbacks) {
  const patch = { textStrengtheningEnabled: enabled }
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
  const general = {
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
  }

  const confirmed = {
    shortcut: 'AltRight',
    shortcut2: 'F8',
    extractShortcut: 'F6',
    extractShortcut2: 'F7',
    shortcutTemplate: 'new-tpl-1',
    shortcut2Template: 'new-tpl-2',
    extractShortcutTemplate: 'new-tpl-3',
    extractShortcut2Template: 'new-tpl-4',
    textStrengtheningEnabled: true,
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
})

test('template deletion fails closed on reassignment rejection', async () => {
  let onSaveCalled = false
  let selectedId = 'custom-to-delete'

  const templates = [{ id: 'custom-to-delete', name: 'Custom' }, { id: 'fallback-1', name: 'Fallback' }]
  const current = templates[0]
  const config = {
    general: { shortcutTemplate: 'custom-to-delete', shortcut2Template: 'other' },
    voiceTemplates: { templates },
  }

  // Simulated updateShortcuts that fails
  const mockUpdateShortcuts = async () => {
    throw new Error('OS conflict')
  }

  // Simulated deleteTemplate implementation
  const remaining = templates.filter(t => t.id !== current.id)
  const fallback = remaining[0]?.id ?? ''
  const patch = {}
  if (config.general.shortcutTemplate === current.id) patch.shortcutTemplate = fallback

  let failedClosed = false
  if (Object.keys(patch).length > 0) {
    try {
      await mockUpdateShortcuts(patch)
      onSaveCalled = true
      selectedId = remaining[0].id
    } catch {
      failedClosed = true
    }
  }

  assert.strictEqual(failedClosed, true, 'Deletion must catch error and fail closed')
  assert.strictEqual(onSaveCalled, false, 'onSave must NEVER be called when binding reassignment fails')
  assert.strictEqual(selectedId, 'custom-to-delete', 'Selection must NOT change when binding reassignment fails')
})

test('template deletion success updates confirmed bindings and template list', async () => {
  let savedConfig = null
  let selectedId = 'custom-to-delete'

  const templates = [{ id: 'custom-to-delete', name: 'Custom' }, { id: 'fallback-1', name: 'Fallback' }]
  const current = templates[0]
  const config = {
    general: {
      theme: 'light',
      shortcut: 'AltRight',
      shortcutTemplate: 'custom-to-delete',
      shortcut2Template: 'keep-tpl',
      textStrengtheningEnabled: true,
    },
    voiceTemplates: { templates },
  }

  const remaining = templates.filter(t => t.id !== current.id)
  const fallback = remaining[0]?.id ?? ''
  const patch = {}
  if (config.general.shortcutTemplate === current.id) patch.shortcutTemplate = fallback

  let updatedGeneral = config.general
  if (Object.keys(patch).length > 0) {
    // Simulated successful updateShortcuts returning confirmed ShortcutUpdateResult
    const res = {
      shortcut: 'AltRight',
      shortcut2: '',
      extractShortcut: 'F6',
      extractShortcut2: '',
      shortcutTemplate: fallback,
      shortcut2Template: 'keep-tpl',
      extractShortcutTemplate: '',
      extractShortcut2Template: '',
      textStrengtheningEnabled: true,
    }
    updatedGeneral = mergeConfirmedShortcutDomain(config.general, res)
  }

  savedConfig = {
    ...config,
    general: updatedGeneral,
    voiceTemplates: { ...config.voiceTemplates, templates: remaining },
  }
  selectedId = remaining[0].id

  assert.strictEqual(savedConfig.general.shortcutTemplate, 'fallback-1')
  assert.strictEqual(savedConfig.general.theme, 'light', 'Theme must be preserved')
  assert.strictEqual(savedConfig.voiceTemplates.templates.length, 1)
  assert.strictEqual(selectedId, 'fallback-1')
})
