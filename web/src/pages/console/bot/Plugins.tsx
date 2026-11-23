import { useRef, useState } from 'react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import * as Tabs from '@radix-ui/react-tabs'
import { ArrowUpCircle, Ellipsis, Eraser, RotateCw, Search, Settings2, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { useBotMutation, useBots, useMe, useUsage } from '@/api/hooks'
import type { Bot, Plugin } from '@/api/types'
import { Button, ConfirmDialog, Input, Skeleton, Switch } from '@/components/ui'
import { cn } from '@/lib/cn'
import { fmtBytes, fmtDateTime } from '@/lib/format'
import { PluginStatusPill } from './status'
import { useErrorToast } from './errors'
import { PluginSettings } from './PluginSettings'
import { PluginMarket } from './PluginMarket'
import { schemaFields } from './schema'

const menuItem =
  'flex min-h-10 cursor-pointer items-center gap-2.5 rounded-lg px-2.5 text-sm text-ink outline-none data-highlighted:bg-paper-2 data-disabled:cursor-not-allowed data-disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0'

type PluginUsage = { memory: number; data: number; deps: number }

function PluginRow({ bot, plugin, usage }: { bot: Bot; plugin: Plugin; usage: PluginUsage | null }) {
  const [confirm, setConfirm] = useState(false)
  const [clearConfirm, setClearConfirm] = useState(false)
  const [disableConfirm, setDisableConfirm] = useState(false)
  const [disableMode, setDisableMode] = useState<'only' | 'clear'>('only')
  const [config, setConfig] = useState(false)
  const configButton = useRef<HTMLButtonElement>(null)
  const actionsButton = useRef<HTMLButtonElement>(null)
  const showError = useErrorToast()
  const toggle = useBotMutation(bot.id, (enabled: boolean) => api.updatePlugin(bot.id, plugin.id, { enabled }))
  const upgrade = useBotMutation(bot.id, () => api.upgradePlugin(bot.id, plugin.id))
  const reload = useBotMutation(bot.id, () => api.reloadPlugin(bot.id, plugin.id))
  const remove = useBotMutation(bot.id, () => api.deletePlugin(bot.id, plugin.id))
  const clear = useBotMutation(bot.id, () => api.clearPluginData(bot.id, plugin.id))
  const name = plugin.display_name || plugin.name
  const enabled = toggle.isPending && toggle.variables !== undefined ? toggle.variables : plugin.enabled
  const installing = plugin.status === 'installing'
  const troubled = plugin.status === 'error' || plugin.status === 'over_quota'
  const busy = toggle.isPending || upgrade.isPending || reload.isPending || remove.isPending || clear.isPending
  const dataSize = usage ? fmtBytes(usage.data) : null
  const hasConfig = schemaFields(plugin.schema ?? {}).some(([, field]) => !field.invisible)
  const canReload = enabled && bot.status === 'running' && !installing && plugin.status !== 'over_quota'

  function reloadPlugin() {
    reload.mutate(undefined, {
      onSuccess: () => toast.success(`${name} 正在重载`),
      onError: (error) => showError(error),
    })
  }

  function setEnabled(value: boolean, then?: () => void) {
    toggle.mutate(value, {
      onSuccess: () => {
        toast.success(value ? `${name} 已启用` : `${name} 已停用`)
        then?.()
      },
      onError: (error) => showError(error),
    })
  }

  function clearData(onDone?: () => void) {
    clear.mutate(undefined, {
      onSuccess: () => {
        onDone?.()
        toast.success(`已清除 ${name} 的数据`)
      },
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
          {(plugin.version || usage) && (
            <p className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[13px] text-ink-3 tabular-nums">
              {plugin.version && <span>{plugin.version}</span>}
              {usage && <span>内存 {fmtBytes(usage.memory)}</span>}
              {usage && <span>数据 {fmtBytes(usage.data)}</span>}
            </p>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2 sm:justify-end">
          <Switch
            checked={enabled}
            disabled={busy || installing}
            label={`启用 ${name}`}
            onCheckedChange={(value) => (value ? setEnabled(true) : setDisableConfirm(true))}
          />
          {hasConfig && (
            <Button
              ref={configButton}
              size="sm"
              variant="secondary"
              className="ml-1 h-10"
              onClick={() => setConfig(true)}
              aria-label={`设置 ${name}`}
            >
              <Settings2 aria-hidden />
              设置
            </Button>
          )}
          <DropdownMenu.Root modal={false}>
            <DropdownMenu.Trigger asChild>
              <Button
                ref={actionsButton}
                variant="ghost"
                className="size-10 px-0"
                aria-busy={busy || undefined}
                aria-label={`${name} 的更多操作`}
              >
                <Ellipsis aria-hidden />
              </Button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                align="end"
                sideOffset={6}
                collisionPadding={12}
                className="z-40 w-52 rounded-xl bg-cream p-1.5 shadow-[var(--shadow-soft)]"
              >
                <DropdownMenu.Item className={menuItem} disabled={busy || !canReload} onSelect={reloadPlugin}>
                  <RotateCw aria-hidden /> 重载
                </DropdownMenu.Item>
                <DropdownMenu.Item
                  className={menuItem}
                  disabled={busy || installing}
                  onSelect={() =>
                    upgrade.mutate(undefined, {
                      onSuccess: () => toast.success(`${name} 正在更新`),
                      onError: (error) => showError(error),
                    })
                  }
                >
                  <ArrowUpCircle aria-hidden /> 更新
                </DropdownMenu.Item>
                <DropdownMenu.Item className={menuItem} disabled={busy || installing} onSelect={() => setClearConfirm(true)}>
                  <Eraser aria-hidden /> 清除数据
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
          {canReload && (
            <Button size="sm" loading={reload.isPending} disabled={busy} onClick={reloadPlugin}>
              重载
            </Button>
          )}
        </div>
      )}
      {config && (
        <PluginSettings bot={bot} plugin={plugin} onClose={() => setConfig(false)} returnFocus={() => configButton.current?.focus()} />
      )}
      <ConfirmDialog
        open={disableConfirm}
        onOpenChange={setDisableConfirm}
        title={`停用 ${name}？`}
        body={
          <p>
            停用后插件的数据会留着，重新启用时接着用。也可以顺便清除数据{dataSize ? `，释放 ${dataSize} 存储` : ''}。
          </p>
        }
        secondaryLabel="只停用"
        secondaryLoading={toggle.isPending && disableMode === 'only'}
        onSecondary={() => {
          setDisableMode('only')
          setEnabled(false, () => setDisableConfirm(false))
        }}
        confirmLabel="停用并清除数据"
        loading={(toggle.isPending && disableMode === 'clear') || clear.isPending}
        onConfirm={() => {
          setDisableMode('clear')
          setEnabled(false, () => clearData(() => setDisableConfirm(false)))
        }}
      />
      <ConfirmDialog
        open={clearConfirm}
        onOpenChange={setClearConfirm}
        returnFocus={() => actionsButton.current?.focus()}
        title={`清除 ${name} 的数据？`}
        body={
          <p>
            会删除这个插件保存的全部数据{dataSize ? `（${dataSize}）` : ''}，比如收藏、记忆和缓存。插件设置会保留，正在运行的插件会自动重启。删除后无法恢复。
          </p>
        }
        confirmLabel="清除数据"
        loading={clear.isPending}
        onConfirm={() => clearData(() => setClearConfirm(false))}
      />
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
  const [tab, setTab] = useState(bot.plugins.length === 0 ? 'market' : 'installed')
  const [search, setSearch] = useState('')
  const { data: me } = useMe()
  const bots = useBots()
  const usage = useUsage()
  const used = bots.data ? bots.data.reduce((n, b) => n + (b.id === bot.id ? bot.plugins.length : b.plugins.length), 0) : bot.plugins.length
  const quota = me?.quota.plugins ?? null
  const full = quota != null && used >= quota
  const empty = bot.plugins.length === 0
  const query = search.trim().toLowerCase()
  const visible = query
    ? bot.plugins.filter((plugin) =>
        [plugin.display_name, plugin.name, plugin.description, plugin.author].some((part) => part.toLowerCase().includes(query)),
      )
    : bot.plugins
  const pluginUsage = (pid: string): PluginUsage | null => {
    if (!usage.data) return null
    const s = usage.data.sources.find((x) => x.bot_id === bot.id && x.plugin_id === pid)
    return { memory: s?.memory_bytes ?? 0, data: s?.storage_bytes ?? 0, deps: s?.deps_bytes ?? 0 }
  }
  function installed() {
    setTab('installed')
    setSearch('')
  }

  return (
    <Tabs.Root value={tab} onValueChange={setTab} className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Tabs.List aria-label="插件视图" className="flex min-w-0 flex-wrap gap-1">
          <Tabs.Trigger
            value="installed"
            className="min-h-10 rounded-lg px-3 text-sm font-semibold text-ink-2 hover:bg-paper-2 data-[state=active]:bg-cream data-[state=active]:text-sky data-[state=active]:ring-1 data-[state=active]:ring-paper-3"
          >
            已安装 <span className="ml-1 tabular-nums">{bot.plugins.length}</span>
          </Tabs.Trigger>
          <Tabs.Trigger
            value="market"
            className="min-h-10 rounded-lg px-3 text-sm font-semibold text-ink-2 hover:bg-paper-2 data-[state=active]:bg-cream data-[state=active]:text-sky data-[state=active]:ring-1 data-[state=active]:ring-paper-3"
          >
            插件市场
          </Tabs.Trigger>
        </Tabs.List>
      </div>
      <Tabs.Content value="installed" className="space-y-3">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          {!empty && (
            <div className="relative min-w-0 flex-1 basis-48">
              <Search className="pointer-events-none absolute top-1/2 left-3.5 size-4 -translate-y-1/2 text-ink-3" aria-hidden />
              <Input
                type="search"
                aria-label="搜索已安装插件"
                placeholder="搜索已安装插件"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                className="h-10 pl-10"
              />
            </div>
          )}
          {quota != null && (
            <p className={full ? 'text-[13px] font-semibold text-warn tabular-nums' : 'text-[13px] text-ink-3 tabular-nums'}>
              账号插件额度 {used} / {quota}
            </p>
          )}
        </div>
        {bots.isLoading && empty ? (
          <Skeleton className="h-24" />
        ) : visible.length > 0 ? (
          <ul className="divide-y divide-paper-3 rounded-2xl border border-paper-3 bg-cream">
            {visible.map((plugin) => (
              <PluginRow key={plugin.id} bot={bot} plugin={plugin} usage={pluginUsage(plugin.id)} />
            ))}
          </ul>
        ) : (
          <p role="status" className="border-t border-paper-3 py-4 text-sm text-ink-2">
            {query && !empty ? '没有匹配的插件。' : '暂无插件。'}
          </p>
        )}
      </Tabs.Content>
      <Tabs.Content value="market">
        <PluginMarket bot={bot} full={full} onInstalled={installed} />
      </Tabs.Content>
    </Tabs.Root>
  )
}
