import { useCallback, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { api, ApiError } from '@/api/client'
import { useBotMutation } from '@/api/hooks'
import type { Bot, Plugin } from '@/api/types'
import { Button, Sheet } from '@/components/ui'
import { SchemaForm } from './SchemaForm'
import { ActionError } from './shared'
import { withDefaults, type Obj } from './schema'

function readSettings(plugin: Plugin) {
  return {
    schema: plugin.schema ?? {},
    value: withDefaults(plugin.schema ?? {}, plugin.config),
    revision: plugin.config_revision,
  }
}

export function PluginSettings({
  bot,
  plugin,
  onClose,
  returnFocus,
}: {
  bot: Bot
  plugin: Plugin
  onClose: () => void
  returnFocus: () => void
}) {
  const [snapshot, setSnapshot] = useState(() => readSettings(plugin))
  const [value, setValue] = useState(snapshot.value)
  const [invalid, setInvalid] = useState<Set<string>>(() => new Set())
  const [formVersion, setFormVersion] = useState(0)
  const [confirmClose, setConfirmClose] = useState(false)
  const save = useBotMutation(bot.id, (config: Obj) => api.updatePlugin(bot.id, plugin.id, { config, config_revision: snapshot.revision }))
  const dirty = invalid.size > 0 || JSON.stringify(value) !== JSON.stringify(snapshot.value)
  const stale = snapshot.revision !== plugin.config_revision
  const conflict = save.error instanceof ApiError && save.error.code === 'plugin_config_changed'
  const name = plugin.display_name || plugin.name

  const setValidity = useCallback((id: string, valid: boolean) => {
    setInvalid((current) => {
      if (current.has(id) === !valid) return current
      const next = new Set(current)
      if (valid) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  useEffect(() => {
    if (!dirty) return
    const warn = (event: BeforeUnloadEvent) => event.preventDefault()
    window.addEventListener('beforeunload', warn)
    return () => window.removeEventListener('beforeunload', warn)
  }, [dirty])

  function requestClose() {
    if (save.isPending) return
    if (dirty) setConfirmClose(true)
    else onClose()
  }

  function reload() {
    const next = readSettings(plugin)
    setSnapshot(next)
    setValue(next.value)
    setInvalid(new Set())
    setFormVersion((version) => version + 1)
    save.reset()
  }

  const description = !plugin.enabled ? '下次启用插件时生效。' : bot.status === 'stopped' ? '下次启动小鱼时生效。' : '保存后重载此插件。'

  return (
    <Sheet
      open
      side="right"
      title={`${name} 设置`}
      description={description}
      onOpenChange={(open) => {
        if (!open) requestClose()
      }}
      returnFocus={returnFocus}
    >
      <form
        className="flex h-full min-h-0 flex-col"
        onSubmit={(event) => {
          event.preventDefault()
          if (!dirty || invalid.size || stale || conflict || confirmClose || save.isPending) return
          save.mutate(value, {
            onSuccess: () => {
              toast.success('插件设置已保存')
              onClose()
            },
          })
        }}
        noValidate
      >
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 py-5 sm:px-8 sm:py-6">
          <fieldset disabled={save.isPending} className="min-w-0">
            <SchemaForm key={formVersion} schema={snapshot.schema} value={value} onChange={setValue} onValidity={setValidity} />
          </fieldset>
        </div>
        <footer className="grid shrink-0 gap-3 border-t border-paper-3 bg-cream px-5 pt-4 pb-[max(1rem,env(safe-area-inset-bottom))] sm:px-8">
          {save.error && !conflict && <ActionError error={save.error} />}
          {confirmClose ? (
            <div className="flex flex-wrap items-center justify-end gap-2">
              <p role="status" className="mr-auto text-sm text-ink">
                放弃未保存的修改？
              </p>
              <Button variant="ghost" onClick={() => setConfirmClose(false)}>
                继续编辑
              </Button>
              <Button variant="danger" onClick={onClose}>
                放弃修改
              </Button>
            </div>
          ) : (
            <>
              {(stale || conflict) && (
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <p role="status" className="text-sm text-ink">
                    重新载入最新设置会清除本次修改。
                  </p>
                  <Button size="sm" onClick={reload} disabled={!stale}>
                    重新载入
                  </Button>
                </div>
              )}
              <div className="flex items-center justify-end gap-2">
                <p role="status" className="mr-auto text-[13px] text-ink-3">
                  {invalid.size > 0 ? '请修正标出的设置项' : ''}
                </p>
                <Button variant="ghost" onClick={requestClose} disabled={save.isPending}>
                  取消
                </Button>
                <Button type="submit" variant="primary" loading={save.isPending} disabled={!dirty || invalid.size > 0 || stale || conflict}>
                  保存设置
                </Button>
              </div>
            </>
          )}
        </footer>
      </form>
    </Sheet>
  )
}
