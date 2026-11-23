import { useState, type ReactNode } from 'react'
import { Eraser, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { useBotMutation, useConversations, useUsage } from '@/api/hooks'
import type { Bot, ConversationSession, Plugin } from '@/api/types'
import { Button, ConfirmDialog, ErrorNote, Skeleton } from '@/components/ui'
import { fmtBytes, fmtDateTime } from '@/lib/format'
import { platformName } from './platform'
import { useErrorToast } from './errors'

const MB = 1024 * 1024

type Target = { kind: 'plugin'; plugin: Plugin; bytes: number } | { kind: 'session'; item: ConversationSession; label: string } | { kind: 'all'; bytes: number }

function Panel({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section className="rounded-2xl border border-paper-3 bg-cream p-5">
      <div className="mb-4 flex flex-wrap items-baseline justify-between gap-3">
        <h2 className="font-display text-lg font-black text-ink">{title}</h2>
        {action}
      </div>
      {children}
    </section>
  )
}

/** Splits `platform_id:MessageType:session_id` into a readable label. */
function sessionLabel(bot: Bot, session: string) {
  const [pid = '', type = '', ...rest] = session.split(':')
  const id = rest.join(':') || session
  const platform = bot.platforms.find((p) => p.id === pid)
  const where = platform ? platformName[platform.kind] : ''
  const kind = type === 'GroupMessage' ? '群聊' : type === 'FriendMessage' ? '私聊' : '会话'
  return { where, kind, id, removed: !platform }
}

export default function Storage({ bot }: { bot: Bot }) {
  const usage = useUsage()
  const conversations = useConversations(bot.id)
  const showError = useErrorToast()
  const [target, setTarget] = useState<Target | null>(null)
  const clearPlugin = useBotMutation(bot.id, (pid: string) => api.clearPluginData(bot.id, pid))
  const clearChats = useBotMutation(bot.id, (session?: string) => api.clearConversations(bot.id, session))

  const sources = usage.data?.sources.filter((s) => s.bot_id === bot.id) ?? []
  const pluginSource = (pid: string) => sources.find((s) => s.plugin_id === pid)
  const chatBytes = sources.find((s) => s.kind === 'conversations')?.storage_bytes ?? 0
  const botBytes = sources.reduce((n, s) => n + s.storage_bytes, 0)
  const sessions = conversations.data?.items ?? []
  const pending = clearPlugin.isPending || clearChats.isPending

  function confirm() {
    if (!target) return
    const done = (message: string) => ({
      onSuccess: () => {
        setTarget(null)
        toast.success(message)
      },
      onError: (e: unknown) => showError(e),
    })
    if (target.kind === 'plugin') {
      clearPlugin.mutate(target.plugin.id, done(`已清除 ${target.plugin.display_name || target.plugin.name} 的数据`))
    } else if (target.kind === 'session') {
      clearChats.mutate(target.item.session, done('已清除这段聊天记录'))
    } else {
      clearChats.mutate(undefined, done('聊天记录已清空'))
    }
  }

  const dialog =
    target?.kind === 'plugin'
      ? {
          title: `清除 ${target.plugin.display_name || target.plugin.name} 的数据？`,
          body: `会删除这个插件保存的全部数据（${fmtBytes(target.bytes)}），比如收藏、记忆和缓存。插件设置会保留，正在运行的插件会自动重启。删除后无法恢复。`,
          label: '清除数据',
        }
      : target?.kind === 'session'
        ? {
            title: `清除${target.label}的聊天记录？`,
            body: `会删除小鱼记住的 ${target.item.messages} 条消息（${fmtBytes(target.item.bytes)}），小鱼在这里会从头聊起。插件自己保存的数据不受影响。删除后无法恢复。`,
            label: '清除记录',
          }
        : {
            title: '清空全部聊天记录？',
            body: `会删除这条小鱼在所有群聊和私聊里的上下文记录（${fmtBytes(target?.kind === 'all' ? target.bytes : 0)}）。插件自己保存的数据不受影响。删除后无法恢复。`,
            label: '全部清空',
          }

  return (
    <div className="grid gap-5">
      <section className="grid gap-1 rounded-2xl border border-paper-3 bg-cream p-5">
        <p className="text-[13px] text-ink-3">这条小鱼计入额度的存储</p>
        {usage.isLoading ? (
          <Skeleton className="h-8 w-40" />
        ) : (
          <p className="font-display text-2xl font-black text-ink tabular-nums">
            {fmtBytes(botBytes)}
            {usage.data && (
              <span className="ml-2 text-sm font-semibold text-ink-3">
                账号合计 {fmtBytes(usage.data.storage_bytes)} / {fmtBytes(usage.data.quota.storage_mb * MB)}
              </span>
            )}
          </p>
        )}
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-2">
          额度只算插件数据和聊天记录。插件的代码和依赖由大家共用，单独列出，不计入额度。
        </p>
      </section>

      <Panel title="插件数据">
        {usage.error ? (
          <ErrorNote error={usage.error} onRetry={() => usage.refetch()} />
        ) : bot.plugins.length === 0 ? (
          <p className="text-sm text-ink-2">还没有安装插件。</p>
        ) : (
          <ul className="divide-y divide-paper-3">
            {bot.plugins.map((plugin) => {
              const s = pluginSource(plugin.id)
              const data = s?.storage_bytes ?? 0
              const name = plugin.display_name || plugin.name
              return (
                <li key={plugin.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 py-3 first:pt-0 last:pb-0">
                  <div className="min-w-0 flex-1 basis-48">
                    <p className="font-semibold wrap-anywhere text-ink">
                      {name}
                      {!plugin.enabled && <span className="ml-2 text-[13px] font-normal text-ink-3">已停用</span>}
                    </p>
                    <p className="mt-0.5 flex flex-wrap gap-x-3 text-[13px] text-ink-3 tabular-nums">
                      <span>
                        数据 <span className="font-semibold text-ink">{usage.isLoading ? '…' : fmtBytes(data)}</span>
                      </span>
                      {(s?.deps_bytes ?? 0) > 0 && <span>代码和依赖 {fmtBytes(s?.deps_bytes)}（不计入）</span>}
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={pending || plugin.status === 'installing'}
                    onClick={() => setTarget({ kind: 'plugin', plugin, bytes: data })}
                    aria-label={`清除 ${name} 的数据`}
                  >
                    <Eraser aria-hidden />
                    清除数据
                  </Button>
                </li>
              )
            })}
          </ul>
        )}
      </Panel>

      <Panel
        title="聊天记录"
        action={
          sessions.length > 0 && (
            <Button size="sm" variant="ghost" className="text-bad" disabled={pending} onClick={() => setTarget({ kind: 'all', bytes: chatBytes })}>
              <Trash2 aria-hidden />
              全部清空
            </Button>
          )
        }
      >
        <p className="mb-3 max-w-prose text-[13px] leading-relaxed text-ink-2">
          小鱼为每个群和每位私聊用户各记最近 200 条消息，用来接上下文。可以按群或按用户分别清除。
        </p>
        {conversations.isLoading ? (
          <Skeleton className="h-24" />
        ) : conversations.error ? (
          <ErrorNote error={conversations.error} onRetry={() => conversations.refetch()} />
        ) : sessions.length === 0 ? (
          <p className="text-sm text-ink-2">还没有聊天记录。</p>
        ) : (
          <ul className="divide-y divide-paper-3">
            {sessions.map((item) => {
              const { where, kind, id, removed } = sessionLabel(bot, item.session)
              const label = `${where}${kind} ${id} `
              return (
                <li key={item.session} className="flex flex-wrap items-center gap-x-4 gap-y-2 py-3 first:pt-0 last:pb-0">
                  <div className="min-w-0 flex-1 basis-48">
                    <p className="flex min-w-0 flex-wrap items-baseline gap-x-2 text-ink">
                      <span className="font-semibold">
                        {where}
                        {kind}
                      </span>
                      <span className="min-w-0 font-mono text-[13px] wrap-anywhere text-ink-2">{id}</span>
                      {removed && <span className="text-[12px] text-ink-3">平台已移除</span>}
                    </p>
                    <p className="mt-0.5 flex flex-wrap gap-x-3 text-[13px] text-ink-3 tabular-nums">
                      <span>{item.messages} 条</span>
                      <span>{fmtBytes(item.bytes)}</span>
                      <span>最近 {fmtDateTime(item.last_ts)}</span>
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={pending}
                    onClick={() => setTarget({ kind: 'session', item, label })}
                    aria-label={`清除${where}${kind} ${id} 的聊天记录`}
                  >
                    <Eraser aria-hidden />
                    清除
                  </Button>
                </li>
              )
            })}
          </ul>
        )}
      </Panel>

      <ConfirmDialog
        open={target !== null}
        onOpenChange={(open) => !open && setTarget(null)}
        title={dialog.title}
        body={<p>{dialog.body}</p>}
        confirmLabel={dialog.label}
        loading={pending}
        onConfirm={confirm}
      />
    </div>
  )
}
