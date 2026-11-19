import { useState, type FormEvent } from 'react'
import { ArrowUpCircle, ExternalLink, Search, Settings2, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { useBotMutation, useBots, useMe, useUsage } from '@/api/hooks'
import type { Bot, Plugin, PluginInspect } from '@/api/types'
import { Button, ConfirmDialog, Field, Input, Sheet, Skeleton, Switch, useFieldId } from '@/components/ui'
import { fmtBytes, fmtDateTime } from '@/lib/format'
import { PluginStatusPill } from './status'
import { ActionError } from './shared'
import { errMsg, useErrorToast } from './errors'
import { SchemaForm } from './SchemaForm'
import { withDefaults } from './schema'

const MARKET = 'https://cloud.astrbot.app/plugin'

function AddPlugin({ bot, used, quota }: { bot: Bot; used: number; quota: number | null }) {
  const [url, setUrl] = useState('')
  const [preview, setPreview] = useState<PluginInspect | null>(null)
  const [inspecting, setInspecting] = useState(false)
  const [inspectErr, setInspectErr] = useState('')
  const add = useBotMutation(bot.id, (repo: string) => api.addPlugin(bot.id, repo))
  const inputId = useFieldId('plugin-url')
  const full = quota != null && used >= quota

  async function inspect(e: FormEvent) {
    e.preventDefault()
    const u = url.trim()
    if (!u) return setInspectErr('先贴一个 GitHub 仓库地址')
    setInspecting(true)
    setInspectErr('')
    setPreview(null)
    add.reset()
    try {
      setPreview(await api.inspectPlugin(u))
    } catch (err) {
      setInspectErr(errMsg(err, '没能读到这个仓库，稍后再试'))
    } finally {
      setInspecting(false)
    }
  }

  function install() {
    if (!preview) return
    add.mutate(preview.repo_url, {
      onSuccess: (p) => {
        toast.success(`${(p as Plugin).display_name || preview.display_name} 开始安装了，装好会自动运行`)
        setPreview(null)
        setUrl('')
      },
    })
  }

  return (
    <section className="rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6" aria-labelledby="plugin-add">
      <h2 id="plugin-add" className="font-display text-lg font-black text-ink">
        装个新插件
      </h2>
      <p className="mt-1 text-sm leading-relaxed text-ink-2">
        去{' '}
        <a href={MARKET} target="_blank" rel="noreferrer" className="inline-flex items-center gap-0.5 font-semibold text-sky hover:underline">
          插件市场
          <ExternalLink className="size-3.5" aria-hidden />
        </a>{' '}
        挑一个，复制它的 GitHub 仓库地址贴到这里。
      </p>
      <form onSubmit={inspect} className="mt-4 flex flex-wrap items-start gap-2" noValidate>
        <Field label="GitHub 仓库地址" htmlFor={inputId} error={inspectErr || undefined} className="min-w-0 flex-1 basis-64">
          <Input
            id={inputId}
            type="url"
            inputMode="url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://github.com/作者/astrbot_plugin_xxx"
            spellCheck={false}
            aria-invalid={!!inspectErr || undefined}
          />
        </Field>
        <Button type="submit" loading={inspecting} className="mt-[26px]">
          <Search aria-hidden />
          查看插件
        </Button>
      </form>

      {preview && (
        <div className="mt-4 animate-[fade-in_150ms_ease-out] rounded-xl border border-paper-3 bg-white p-4">
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
            <h3 className="font-semibold text-ink">{preview.display_name || preview.name}</h3>
            <span className="text-[13px] text-ink-3 tabular-nums">{preview.version}</span>
            <span className="text-[13px] text-ink-3">作者 {preview.author}</span>
          </div>
          <p className="mt-1 font-mono text-[12px] break-all text-ink-3">{preview.name}</p>
          <p className="mt-2 text-sm leading-relaxed text-ink-2">{preview.description || '作者没有写简介。'}</p>
          <ul className="mt-2 grid gap-0.5 text-[13px] text-ink-3">
            {preview.has_requirements && <li>需要下载依赖，安装可能要等一两分钟。</li>}
            {preview.schema && <li>装好后可以在这里改它的设置。</li>}
          </ul>
          {add.error && (
            <div className="mt-3">
              <ActionError error={add.error} />
            </div>
          )}
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <Button variant="primary" onClick={install} loading={add.isPending} disabled={full}>
              装上这个插件
            </Button>
            <Button variant="ghost" onClick={() => setPreview(null)}>
              换一个
            </Button>
            {full && <p className="basis-full text-[13px] text-warn">插件额度已经用完了，先移除一个不用的再装。</p>}
          </div>
        </div>
      )}
    </section>
  )
}

function ConfigSheet({ bot, plugin, open, onOpenChange }: { bot: Bot; plugin: Plugin; open: boolean; onOpenChange: (v: boolean) => void }) {
  const [value, setValue] = useState(() => withDefaults(plugin.schema ?? {}, plugin.config ?? {}))
  const [invalid, setInvalid] = useState<Record<string, boolean>>({})
  const save = useBotMutation(bot.id, (config: Record<string, unknown>) => api.updatePlugin(bot.id, plugin.id, { config }))
  const showError = useErrorToast()
  const blocked = Object.values(invalid).some(Boolean)

  return (
    <Sheet
      open={open}
      onOpenChange={onOpenChange}
      title={`${plugin.display_name || plugin.name} 的设置`}
      description="保存后插件会用新的设置重新加载。"
    >
      <form
        onSubmit={(e) => {
          e.preventDefault()
          if (blocked) return
          save.mutate(value, {
            onSuccess: () => {
              toast.success('插件设置保存好了')
              onOpenChange(false)
            },
            onError: (err) => showError(err, '没能保存设置'),
          })
        }}
        noValidate
      >
        <SchemaForm
          schema={plugin.schema ?? {}}
          value={value}
          onChange={setValue}
          onValidity={(k, ok) => setInvalid((m) => ({ ...m, [k]: !ok }))}
        />
        <div className="sticky -bottom-6 -mx-6 mt-6 flex flex-wrap items-center justify-end gap-2 border-t border-paper-3 bg-paper px-6 py-4">
          {blocked && <p className="mr-auto text-[13px] text-bad">有几项没填对，改好才能保存</p>}
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button type="submit" variant="primary" loading={save.isPending} disabled={blocked}>
            保存设置
          </Button>
        </div>
      </form>
    </Sheet>
  )
}

function PluginRow({ bot, plugin, memory }: { bot: Bot; plugin: Plugin; memory: number | null }) {
  const [confirm, setConfirm] = useState(false)
  const [config, setConfig] = useState(false)
  const showError = useErrorToast()
  const toggle = useBotMutation(bot.id, (enabled: boolean) => api.updatePlugin(bot.id, plugin.id, { enabled }))
  const upgrade = useBotMutation(bot.id, () => api.upgradePlugin(bot.id, plugin.id))
  const remove = useBotMutation(bot.id, () => api.deletePlugin(bot.id, plugin.id))
  const name = plugin.display_name || plugin.name
  const enabled = toggle.isPending && toggle.variables !== undefined ? toggle.variables : plugin.enabled
  const installing = plugin.status === 'installing'
  const troubled = plugin.status === 'error' || plugin.status === 'over_quota'
  const hasConfig = !!plugin.schema && Object.values(plugin.schema).some((f) => !f.invisible)

  return (
    <li className="py-4 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-start gap-x-4 gap-y-3">
        <div className="min-w-0 flex-1 basis-60">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="font-semibold text-ink">{name}</h3>
            <PluginStatusPill status={plugin.status} />
          </div>
          <p className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[13px] text-ink-3">
            <span className="tabular-nums">{plugin.version}</span>
            <span>作者 {plugin.author}</span>
            <span className="tabular-nums">内存 {memory == null ? '读取中' : fmtBytes(memory)}</span>
            <span className="tabular-nums">更新于 {fmtDateTime(plugin.updated_at)}</span>
            <a href={plugin.repo_url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-0.5 text-sky hover:underline">
              仓库
              <ExternalLink className="size-3" aria-hidden />
            </a>
          </p>
          {plugin.description && <p className="mt-1.5 text-sm leading-relaxed text-ink-2">{plugin.description}</p>}
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <label className="mr-1 flex items-center gap-2 text-[13px] font-semibold text-ink-2">
            <Switch
              checked={enabled}
              disabled={toggle.isPending || installing}
              label={`启用 ${name}`}
              onCheckedChange={(v) =>
                toggle.mutate(v, { onSuccess: () => toast.success(v ? `${name} 已启用` : `${name} 已停用，不再占内存`), onError: (e) => showError(e) })
              }
            />
            {enabled ? '开' : '关'}
          </label>
          {hasConfig && (
            <Button size="sm" variant="secondary" onClick={() => setConfig(true)}>
              <Settings2 aria-hidden />
              设置
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            loading={upgrade.isPending}
            disabled={installing}
            onClick={() =>
              upgrade.mutate(undefined, { onSuccess: () => toast.success(`正在把 ${name} 更新到最新版`), onError: (e) => showError(e) })
            }
          >
            <ArrowUpCircle aria-hidden />
            更新
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setConfirm(true)} aria-label={`移除 ${name}`}>
            <Trash2 aria-hidden />
            移除
          </Button>
        </div>
      </div>
      {troubled && (
        <p role="status" className="mt-3 rounded-xl bg-bad-soft px-3.5 py-2.5 text-[13px] leading-relaxed text-bad">
          {plugin.status_detail ||
            (plugin.status === 'over_quota' ? '这个插件吃得太多，超出了内存额度，先停下了。' : '插件运行出错了，看看日志里的报错。')}
        </p>
      )}
      {config && <ConfigSheet bot={bot} plugin={plugin} open={config} onOpenChange={setConfig} />}
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`移除 ${name}？`}
        body={<p>插件会停下并删除，它的设置和保存的数据也会清掉。之后想用可以重新安装。</p>}
        confirmLabel="确认移除"
        loading={remove.isPending}
        onConfirm={() =>
          remove.mutate(undefined, {
            onSuccess: () => {
              setConfirm(false)
              toast.success(`已移除 ${name}`)
            },
            onError: (e) => showError(e),
          })
        }
      />
    </li>
  )
}

export default function Plugins({ bot }: { bot: Bot }) {
  const { data: me } = useMe()
  const bots = useBots()
  const usage = useUsage()
  const used = bots.data ? bots.data.reduce((n, b) => n + (b.id === bot.id ? bot.plugins.length : b.plugins.length), 0) : bot.plugins.length
  const quota = me?.quota.plugins ?? null
  const mem = (pid: string) => {
    if (!usage.data) return null
    return usage.data.sources.find((s) => s.bot_id === bot.id && s.plugin_id === pid)?.memory_bytes ?? 0
  }

  return (
    <div className="grid gap-6">
      <AddPlugin bot={bot} used={used} quota={quota} />

      <section aria-labelledby="plugin-list">
        <div className="mb-3 flex flex-wrap items-baseline justify-between gap-2">
          <h2 id="plugin-list" className="font-display text-lg font-black text-ink">
            已安装
            <span className="ml-2 text-sm font-semibold text-ink-3 tabular-nums">{bot.plugins.length}</span>
          </h2>
          {quota != null && (
            <p className={used >= quota ? 'text-[13px] font-semibold text-warn tabular-nums' : 'text-[13px] text-ink-3 tabular-nums'}>
              插件额度 {used} / {quota}，所有小鱼一起算
            </p>
          )}
        </div>
        {bots.isLoading && !bot.plugins.length ? (
          <Skeleton className="h-24" />
        ) : bot.plugins.length === 0 ? (
          <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-sm leading-relaxed text-ink-2">
            还没装插件。插件能让小鱼多会几样本事，比如每日一言、表情包、查天气。从插件市场复制仓库地址，贴到上面就能装。
          </p>
        ) : (
          <ul className="divide-y divide-paper-3 rounded-2xl border border-paper-3 bg-cream p-5">
            {bot.plugins.map((p) => (
              <PluginRow key={p.id} bot={bot} plugin={p} memory={mem(p.id)} />
            ))}
          </ul>
        )}
      </section>
    </div>
  )
}
