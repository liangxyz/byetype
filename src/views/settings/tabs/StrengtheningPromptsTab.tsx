import { AppConfig, ShortcutUpdateResult } from '../../../core/types'
import { TemplatePromptsPanel } from '../components/TemplatePromptsPanel'

interface Props {
  config: AppConfig
  onSave: (config: AppConfig) => void
  onDeleteTemplate?: (
    kind: 'voice' | 'image',
    deletedId: string,
    confirmedDomain?: ShortcutUpdateResult,
  ) => void
}

export function StrengtheningPromptsTab({ config, onSave, onDeleteTemplate }: Props) {
  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0, overflow: 'auto' }}>
      <h2 className="content-title optimization-heading" style={{ flexShrink: 0 }}>文本强化规则</h2>
      <p className="rules-intro">按输出风格处理识别文本，例如分段、翻译。</p>
      <TemplatePromptsPanel kind="voice" config={config} onSave={onSave} onDeleteTemplate={onDeleteTemplate} />
    </div>
  )
}
