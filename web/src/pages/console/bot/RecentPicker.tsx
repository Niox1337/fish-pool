import { useState, type ReactNode } from 'react'
import { Check, History, Plus } from 'lucide-react'
import { useRecent } from '@/api/hooks'
import type { Bot, RecentChats } from '@/api/types'
import { Button, ErrorNote, Skeleton } from '@/components/ui'
import { fmtAgo } from '@/lib/format'
import { platformName } from './platform'
import { RecentBotContext, type RecentKind } from './recent'

/** Makes the bot available to list fields rendered inside a plugin settings form. */
export function RecentBotProvider({ bot, children }: { bot: Bot; children: ReactNode }) {
  return <RecentBotContext.Provider value={bot}>{children}</RecentBotContext.Provider>
}

type Row = { value: string; label: string; detail: string; ts: number }

function splitOrigin(origin: string) {
  const [platformId = '', type = '', ...rest] = origin.split(':')
  return { platformId, group: type === 'GroupMessage', id: rest.join(':') }
}

function rowsOf(bot: Bot, data: RecentChats, kind: RecentKind, format: (id: string, platformKind: string) => string): Row[] {
  const where = (platformId: string) => {
    const platform = bot.platforms.find((p) => p.id === platformId)
    return platform ? { name: platformName[platform.kind], kind: platform.kind as string } : { name: '已移除的平台', kind: '' }
  }
  if (kind === 'user') {
    return data.users.map((entry) => {
      const origin = splitOrigin(entry.origin)
      const platform = where(origin.platformId)
      return {
        value: format(entry.id, platform.kind),
        label: `${platform.name} · 最近在${origin.group ? '群聊' : '私聊'}里发言`,
        detail: entry.id,
        ts: entry.last_ts,
      }
    })
  }
  return data.sessions.flatMap((entry) => {
    const origin = splitOrigin(entry.id)
    if ((kind === 'group' || kind === 'group_session') && !origin.group) return []
    if (kind === 'friend_session' && origin.group) return []
    return [
      {
        value: kind === 'group' ? origin.id : entry.id,
        label: `${where(origin.platformId).name} · ${origin.group ? '群聊' : '私聊'}`,
        detail: origin.id,
        ts: entry.last_ts,
      },
    ]
  })
}

const nouns: Record<RecentKind, string> = {
  session: '会话',
  friend_session: '私聊',
  group_session: '群聊',
  group: '群聊',
  user: '账号',
}

/** Lists sessions or senders that recently messaged the bot, each with a button that adds it to a list setting. */
export function RecentPicker({
  bot,
  kind,
  values,
  onAdd,
  format = (id) => id,
}: {
  bot: Bot
  kind: RecentKind
  values: readonly unknown[]
  onAdd: (value: string) => void
  format?: (id: string, platformKind: string) => string
}) {
  const [open, setOpen] = useState(false)
  const recent = useRecent(bot.id, open)
  const rows = recent.data ? rowsOf(bot, recent.data, kind, format) : []
  const noun = nouns[kind]

  return (
    <div className="grid gap-2">
      <Button variant="ghost" size="sm" className="justify-self-start" aria-expanded={open} onClick={() => setOpen(!open)}>
        <History aria-hidden />
        {open ? '收起最近聊天' : `从最近聊天选择${noun}`}
      </Button>
      {open && (
        <div className="rounded-xl border border-paper-3 bg-cream/60 p-3">
          {recent.isPending ? (
            <Skeleton className="h-16" />
          ) : recent.isError ? (
            <ErrorNote error={recent.error} onRetry={() => recent.refetch()} />
          ) : rows.length === 0 ? (
            <p className="text-[13px] leading-relaxed text-ink-2">还没有记录。让对方先给小鱼发一条消息，再回来看看。</p>
          ) : (
            <ul className="divide-y divide-paper-3">
              {rows.map((row) => {
                const added = values.includes(row.value)
                return (
                  <li key={row.value} className="flex items-center gap-3 py-2 first:pt-0 last:pb-0">
                    <div className="min-w-0 flex-1">
                      <p className="text-[13px] font-semibold text-ink">{row.label}</p>
                      <p className="truncate font-mono text-xs text-ink-3" title={row.value}>
                        {row.detail}
                      </p>
                    </div>
                    <span className="hidden shrink-0 text-xs text-ink-3 tabular-nums sm:inline">{fmtAgo(row.ts)}</span>
                    <Button variant="secondary" size="sm" className="shrink-0" disabled={added} onClick={() => onAdd(row.value)}>
                      {added ? <Check aria-hidden /> : <Plus aria-hidden />}
                      {added ? '已添加' : '添加'}
                    </Button>
                  </li>
                )
              })}
            </ul>
          )}
          <p className="mt-3 text-xs leading-relaxed text-ink-3">
            这里只列出最近给小鱼发过消息的{noun}。也可以在聊天里发送 /sid，查看当前会话和自己的账号。
          </p>
        </div>
      )}
    </div>
  )
}
