import { useRef, useState, type FormEvent } from 'react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { ArrowUpCircle, Ellipsis, ExternalLink, Plus, RotateCw, Search, Settings2, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { useBotMutation, useBots, useMe, useUsage } from '@/api/hooks'
import type { Bot, Plugin, PluginInspect } from '@/api/types'
import { Button, ConfirmDialog, Field, Input, Skeleton, Switch, useFieldId } from '@/components/ui'
import { Pupil } from '@/components/Pupil'
import { cn } from '@/lib/cn'
import { fmtBytes, fmtDateTime } from '@/lib/format'
import { PluginStatusPill } from './status'
import { ActionError } from './shared'
import { errMsg, useErrorToast } from './errors'
import { PluginSettings } from './PluginSettings'
import { schemaFields } from './schema'

const MARKET = 'https://cloud.astrbot.app/market'
const GENGWIKI_PLUGIN = 'https://github.com/Niox1337/astrbot_plugin_gengwiki'

function AddPlugin({ bot, full, onInstalled }: { bot: Bot; full: boolean; onInstalled: () => void }) {
  const [url, setUrl] = useState('')
  const [preview, setPreview] = useState<PluginInspect | null>(null)
  const [inspecting, setInspecting] = useState(false)
  const [inspectErr, setInspectErr] = useState('')
  const add = useBotMutation(bot.id, (repo: string) => api.addPlugin(bot.id, repo))
  const inputId = useFieldId('plugin-url')

  async function inspect(repo: string) {
    const u = repo.trim()
    if (!u) return setInspectErr('先贴一个 GitHub 仓库地址')
    setUrl(u)
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

  function submit(event: FormEvent) {
    event.preventDefault()
    void inspect(url)
  }

  function install() {
    if (!preview) return
    add.mutate(preview.repo_url, {
      onSuccess: (p) => {
        toast.success(`${(p as Plugin).display_name || preview.display_name || preview.name} 正在安装`)
        setPreview(null)
        setUrl('')
        onInstalled()
      },
    })
  }

  return (
    <section id="plugin-install" className="rounded-2xl border border-paper-3 bg-cream p-4 sm:p-5" aria-labelledby="plugin-add">
      <h3 id="plugin-add" className="sr-only">通过 GitHub 安装</h3>
      <form onSubmit={submit} className="grid items-start gap-2 sm:grid-cols-[minmax(0,1fr)_auto]" noValidate>
        <Field label="GitHub 仓库地址" htmlFor={inputId} error={inspectErr || undefined} className="min-w-0 flex-1 basis-64">
          <Input
            id={inputId}
            type="url"
            inputMode="url"
            value={url}
            disabled={inspecting || add.isPending}
            onChange={(e) => {
              setUrl(e.target.value)
              setPreview(null)
              setInspectErr('')
            }}
            placeholder="https://github.com/作者/astrbot_plugin_xxx"
            spellCheck={false}
            aria-invalid={!!inspectErr || undefined}
          />
        </Field>
        <Button type="submit" loading={inspecting} disabled={add.isPending} className="h-11 justify-self-start sm:mt-[26px]">
          查看
        </Button>
      </form>

      {!preview && !bot.plugins.some((plugin) => plugin.name === 'astrbot_plugin_gengwiki') && (
        <aside aria-label="梗Wiki 插件推荐" className="mt-4 flex flex-wrap items-center gap-x-3 gap-y-2 border-t border-paper-3 pt-4">
          <Pupil mood="happy" size={44} />
          <div className="min-w-0 flex-1 basis-40">
            <p className="font-semibold text-ink">给小鱼加点梗</p>
            <p className="mt-0.5 text-sm leading-relaxed text-ink-2">
              装上 <a href="https://gengwiki.com/" target="_blank" rel="noreferrer" className="font-semibold text-sky underline-offset-2 hover:underline">梗Wiki</a>，群友抛梗，小鱼接梗，顺手甩张表情包。
            </p>
          </div>
          <Button variant="ghost" size="sm" className="h-10" disabled={inspecting || add.isPending} onClick={() => { void inspect(GENGWIKI_PLUGIN) }}>
            查看梗Wiki 插件
          </Button>
        </aside>
      )}

      {preview && (
        <div className="mt-4 border-t border-paper-3 pt-4">
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
            <h3 className="font-semibold text-ink">{preview.display_name || preview.name}</h3>
            <span className="text-[13px] text-ink-3 tabular-nums">{preview.version}</span>
            <span className="text-[13px] text-ink-3">作者 {preview.author}</span>
          </div>
          {preview.display_name && preview.display_name !== preview.name && <p className="mt-1 font-mono text-[12px] break-all text-ink-3">{preview.name}</p>}
          {preview.description && <p className="mt-2 text-sm leading-relaxed text-ink-2">{preview.description}</p>}
          {preview.has_requirements && <p className="mt-2 text-[13px] text-ink-3">安装时需下载依赖。</p>}
          {add.error && (
            <div className="mt-3">
              <ActionError error={add.error} />
            </div>
          )}
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <Button variant="primary" onClick={install} loading={add.isPending} disabled={full}>
              确认安装
            </Button>
            {full && <p className="basis-full text-[13px] text-warn">额度已满。请先移除一个插件。</p>}
          </div>
        </div>
      )}
    </section>
  )
}

const menuItem = 'flex min-h-10 cursor-pointer items-center gap-2.5 rounded-lg px-2.5 text-sm text-ink outline-none data-highlighted:bg-paper-2 data-disabled:cursor-not-allowed data-disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0'

function PluginRow({ bot, plugin, memory }: { bot: Bot; plugin: Plugin; memory: number | null }) {
  const [confirm, setConfirm] = useState(false)
  const [config, setConfig] = useState(false)
  const configButton = useRef<HTMLButtonElement>(null)
  const actionsButton = useRef<HTMLButtonElement>(null)
  const showError = useErrorToast()
  const toggle = useBotMutation(bot.id, (enabled: boolean) => api.updatePlugin(bot.id, plugin.id, { enabled }))
  const upgrade = useBotMutation(bot.id, () => api.upgradePlugin(bot.id, plugin.id))
  const reload = useBotMutation(bot.id, () => api.reloadPlugin(bot.id, plugin.id))
  const remove = useBotMutation(bot.id, () => api.deletePlugin(bot.id, plugin.id))
  const name = plugin.display_name || plugin.name
  const enabled = toggle.isPending && toggle.variables !== undefined ? toggle.variables : plugin.enabled
  const installing = plugin.status === 'installing'
  const troubled = plugin.status === 'error' || plugin.status === 'over_quota'
  const busy = toggle.isPending || upgrade.isPending || reload.isPending || remove.isPending
  const hasConfig = schemaFields(plugin.schema ?? {}).some(([, field]) => !field.invisible)
  const canReload = enabled && bot.status === 'running' && !installing && plugin.status !== 'over_quota'

  function reloadPlugin() {
    reload.mutate(undefined, {
      onSuccess: () => toast.success(`${name} 正在重载`),
      onError: (error) => showError(error),
    })
  }

  return (
    <li className="px-4 py-4 sm:px-5">
      <div className="grid items-center gap-3 sm:grid-cols-[minmax(0,1fr)_auto] sm:gap-5">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
            <h3 className="font-semibold wrap-anywhere text-ink">{name}</h3>
            <PluginStatusPill status={plugin.status} />
          </div>
          {plugin.description && <p className="mt-1 max-w-prose text-sm leading-relaxed wrap-anywhere text-ink-2">{plugin.description}</p>}
          {(plugin.version || memory != null) && (
            <p className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[13px] text-ink-3 tabular-nums">
              {plugin.version && <span>{plugin.version}</span>}
              {memory != null && <span>内存 {fmtBytes(memory)}</span>}
            </p>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2 sm:justify-end">
          <Switch
            checked={enabled}
            disabled={busy || installing}
            label={`启用 ${name}`}
            onCheckedChange={(value) => toggle.mutate(value, {
              onSuccess: () => toast.success(value ? `${name} 已启用` : `${name} 已停用`),
              onError: (error) => showError(error),
            })}
          />
          {hasConfig && (
            <Button ref={configButton} size="sm" variant="secondary" className="ml-1 h-10" onClick={() => setConfig(true)} aria-label={`设置 ${name}`}>
              <Settings2 aria-hidden />
              设置
            </Button>
          )}
          <DropdownMenu.Root modal={false}>
            <DropdownMenu.Trigger asChild>
              <Button ref={actionsButton} variant="ghost" className="size-10 px-0" aria-busy={busy || undefined} aria-label={`${name} 的更多操作`}>
                <Ellipsis aria-hidden />
              </Button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content align="end" sideOffset={6} collisionPadding={12} className="z-40 w-52 rounded-xl bg-cream p-1.5 shadow-[var(--shadow-soft)]">
                <DropdownMenu.Item className={menuItem} disabled={busy || !canReload} onSelect={reloadPlugin}>
                  <RotateCw aria-hidden /> 重载
                </DropdownMenu.Item>
                <DropdownMenu.Item className={menuItem} disabled={busy || installing} onSelect={() => upgrade.mutate(undefined, {
                  onSuccess: () => toast.success(`${name} 正在更新`),
                  onError: (error) => showError(error),
                })}>
                  <ArrowUpCircle aria-hidden /> 更新
                </DropdownMenu.Item>
                <DropdownMenu.Item asChild className={menuItem}>
                  <a href={plugin.repo_url} target="_blank" rel="noreferrer"><ExternalLink aria-hidden /> 仓库</a>
                </DropdownMenu.Item>
                <DropdownMenu.Label className="px-2.5 py-1 text-xs leading-relaxed wrap-anywhere text-ink-3">
                  {plugin.author && <span className="block">作者 {plugin.author}</span>}
                  {plugin.commit && <span className="block font-mono">提交 {plugin.commit.slice(0, 7)}</span>}
                  <span className="block">每小时自动检查更新</span>
                  <span className="block tabular-nums">最近变化 {fmtDateTime(plugin.updated_at)}</span>
                </DropdownMenu.Label>
                <DropdownMenu.Separator className="my-1 h-px bg-paper-3" />
                <DropdownMenu.Item className={cn(menuItem, 'text-bad')} disabled={busy} onSelect={() => setConfirm(true)}>
                  <Trash2 aria-hidden /> 卸载
                </DropdownMenu.Item>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
      </div>
      {troubled && (
        <div className="mt-3 flex flex-wrap items-center justify-between gap-2 rounded-xl bg-bad-soft px-3.5 py-2">
          <p role="status" className="min-w-0 flex-1 basis-60 text-[13px] leading-relaxed wrap-anywhere text-bad">
            {plugin.status_detail || (plugin.status === 'over_quota' ? '插件超出内存配额。' : '插件出错，请查看日志。')}
          </p>
          {canReload && <Button size="sm" loading={reload.isPending} disabled={busy} onClick={reloadPlugin}>重载</Button>}
        </div>
      )}
      {config && <PluginSettings bot={bot} plugin={plugin} onClose={() => setConfig(false)} returnFocus={() => configButton.current?.focus()} />}
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        returnFocus={() => actionsButton.current?.focus()}
        title={`卸载 ${name}？`}
        body={<p>此插件的设置和数据将一并删除。</p>}
        confirmLabel="确认卸载"
        loading={remove.isPending}
        onConfirm={() =>
          remove.mutate(undefined, {
            onSuccess: () => {
              setConfirm(false)
              toast.success(`已卸载 ${name}`)
            },
            onError: (e) => showError(e),
          })
        }
      />
    </li>
  )
}

export default function Plugins({ bot }: { bot: Bot }) {
  const [showInstall, setShowInstall] = useState(bot.plugins.length === 0)
  const [search, setSearch] = useState('')
  const { data: me } = useMe()
  const bots = useBots()
  const usage = useUsage()
  const used = bots.data ? bots.data.reduce((n, b) => n + (b.id === bot.id ? bot.plugins.length : b.plugins.length), 0) : bot.plugins.length
  const quota = me?.quota.plugins ?? null
  const full = quota != null && used >= quota
  const empty = bot.plugins.length === 0
  const query = search.trim().toLowerCase()
  const visible = query ? bot.plugins.filter((plugin) =>
    [plugin.display_name, plugin.name, plugin.description, plugin.author].some((part) => part.toLowerCase().includes(query)),
  ) : bot.plugins
  const mem = (pid: string) => {
    if (!usage.data) return null
    return usage.data.sources.find((s) => s.bot_id === bot.id && s.plugin_id === pid)?.memory_bytes ?? 0
  }

  return (
    <section aria-labelledby="plugin-list" className="grid gap-4">
      <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-2 lg:grid-cols-[minmax(0,1fr)_minmax(10rem,16rem)_auto]">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <h2 id="plugin-list" className="font-display text-lg font-black text-ink">
            已安装
            <span className="ml-2 text-sm font-semibold text-ink-3 tabular-nums">{' '}{bot.plugins.length}</span>
          </h2>
          {quota != null && (
            <p className={full ? 'text-[13px] font-semibold text-warn tabular-nums' : 'text-[13px] text-ink-3 tabular-nums'}>
              账号插件额度 {used} / {quota}
            </p>
          )}
        </div>
        {!empty && (
          <div className="relative col-span-2 row-start-2 lg:col-span-1 lg:col-start-2 lg:row-start-1">
            <Search className="pointer-events-none absolute top-1/2 left-3.5 size-4 -translate-y-1/2 text-ink-3" aria-hidden />
            <Input type="search" aria-label="搜索已安装插件" placeholder="搜索已安装插件" value={search} onChange={(event) => setSearch(event.target.value)} className="h-10 pl-10" />
          </div>
        )}
        <div className="col-start-2 row-start-1 flex flex-wrap items-center justify-end gap-2 lg:col-start-3">
          <a href={MARKET} target="_blank" rel="noreferrer" className="inline-flex min-h-10 items-center gap-1.5 rounded-lg px-2 text-sm font-semibold text-sky hover:underline">
            插件市场 <ExternalLink className="size-4" aria-hidden />
          </a>
          <Button variant={showInstall ? 'secondary' : 'primary'} aria-expanded={showInstall} aria-controls={showInstall ? 'plugin-install' : undefined} onClick={() => setShowInstall(!showInstall)}>
            {!showInstall && <Plus aria-hidden />}
            {showInstall ? '收起安装' : '安装插件'}
          </Button>
        </div>
      </div>
      {showInstall && <AddPlugin bot={bot} full={full} onInstalled={() => { setShowInstall(false); setSearch('') }} />}
      {bots.isLoading && empty ? (
        <Skeleton className="h-24" />
      ) : visible.length > 0 ? (
        <ul className="divide-y divide-paper-3 rounded-2xl border border-paper-3 bg-cream">
          {visible.map((plugin) => (
            <PluginRow key={plugin.id} bot={bot} plugin={plugin} memory={mem(plugin.id)} />
          ))}
        </ul>
      ) : !showInstall || query ? (
        <p role="status" className="border-t border-paper-3 py-4 text-sm text-ink-2">{query && !empty ? '没有匹配的插件。' : '暂无插件。'}</p>
      ) : null}
    </section>
  )
}
