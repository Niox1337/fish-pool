import { useState } from 'react'
import { Link, Navigate, NavLink, useParams } from 'react-router'
import { ChevronLeft, LayoutGrid, Link2, Play, Puzzle, RotateCw, ScrollText, Square, UserRoundPen } from 'lucide-react'
import { toast } from 'sonner'
import { api, ApiError } from '@/api/client'
import { useBot, useBotMutation } from '@/api/hooks'
import type { Bot, BotSettings } from '@/api/types'
import { Button, ErrorNote, Skeleton } from '@/components/ui'
import { Pupil, statusMood } from '@/components/Pupil'
import { cn } from '@/lib/cn'
import { BotStatusPill } from './status'
import { ActionError, InlineName } from './shared'
import Overview from './Overview'
import Connect from './Connect'
import Persona from './Persona'
import Plugins from './Plugins'
import Logs from './Logs'

const TABS = [
  { key: 'overview', label: '概览', icon: LayoutGrid },
  { key: 'connect', label: '接入', icon: Link2 },
  { key: 'persona', label: '人设与模型', icon: UserRoundPen },
  { key: 'plugins', label: '插件', icon: Puzzle },
  { key: 'logs', label: '日志', icon: ScrollText },
] as const
type TabKey = (typeof TABS)[number]['key']

function BackLink() {
  return (
    <Link to="/app" className="mb-4 inline-flex items-center gap-1 rounded-lg text-sm font-semibold text-ink-2 hover:text-ink">
      <ChevronLeft className="size-4" aria-hidden />
      我的小鱼
    </Link>
  )
}

function Header({ bot }: { bot: Bot }) {
  const action = useBotMutation(bot.id, (a: 'start' | 'stop' | 'restart') => api.botAction(bot.id, a))
  const rename = useBotMutation(bot.id, (name: string) => api.updateBot(bot.id, { name }))
  const busy = action.isPending ? action.variables : null

  function run(a: 'start' | 'stop' | 'restart') {
    action.mutate(a, {
      onSuccess: () => toast.success(a === 'stop' ? `${bot.name} 下课睡觉了` : a === 'start' ? `${bot.name} 正在入座` : `${bot.name} 正在重新入座`),
    })
  }

  const asleep = bot.status === 'stopped'
  const troubled = bot.status === 'error'

  return (
    <header className="mb-5">
      <div className="flex flex-wrap items-center gap-4">
        <Pupil mood={statusMood[bot.status]} size={88} className="rounded-3xl bg-paper-2" />
        <div className="min-w-0 flex-1 basis-48">
          <InlineName value={bot.name} label="小鱼名字" onSave={(name) => rename.mutateAsync(name)}>
            <h1 className="truncate font-display text-2xl font-black text-ink sm:text-[28px]">{bot.name}</h1>
          </InlineName>
          <div className="mt-1.5 flex flex-wrap items-center gap-2">
            <BotStatusPill status={bot.status} />
            {bot.status_detail && bot.status !== 'running' && <p className="text-[13px] leading-relaxed text-ink-2">{bot.status_detail}</p>}
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {asleep ? (
            <Button variant="primary" loading={busy === 'start'} disabled={!!busy} onClick={() => run('start')}>
              <Play aria-hidden />
              启动
            </Button>
          ) : (
            <>
              <Button
                variant={troubled ? 'primary' : 'secondary'}
                loading={busy === 'restart'}
                disabled={!!busy || bot.status === 'starting'}
                onClick={() => run('restart')}
              >
                <RotateCw aria-hidden />
                重启
              </Button>
              <Button variant="secondary" loading={busy === 'stop'} disabled={!!busy} onClick={() => run('stop')}>
                <Square aria-hidden />
                停止
              </Button>
            </>
          )}
        </div>
      </div>
      {action.error && (
        <div className="mt-4">
          <ActionError error={action.error} onDismiss={() => action.reset()} />
        </div>
      )}
    </header>
  )
}

function Tabs({ botId, dirty }: { botId: string; dirty: boolean }) {
  return (
    <nav aria-label="小鱼设置" className="-mx-4 mb-6 overflow-x-auto overflow-y-hidden px-4 sm:mx-0 sm:px-0">
      <ul className="flex min-w-max gap-1 shadow-[inset_0_-1px_0_var(--color-paper-3)]">
        {TABS.map((t) => (
          <li key={t.key}>
            <NavLink
              to={t.key === 'overview' ? `/app/bots/${botId}` : `/app/bots/${botId}/${t.key}`}
              end
              className={({ isActive }) =>
                cn(
                  'flex h-11 items-center gap-1.5 border-b-2 px-3 text-[15px] font-semibold transition-colors duration-150',
                  isActive ? 'border-cobalt text-ink' : 'border-transparent text-ink-3 hover:text-ink',
                )
              }
            >
              <t.icon className="size-4" aria-hidden />
              {t.label}
              {t.key === 'persona' && dirty && (
                <span className="size-2 rounded-full bg-pink-ink" role="img" aria-label="有没保存的修改" />
              )}
            </NavLink>
          </li>
        ))}
      </ul>
    </nav>
  )
}

export default function BotPage() {
  const { botId = '' } = useParams()
  return <BotView key={botId} />
}

function BotView() {
  const { botId = '', tab } = useParams()
  const bot = useBot(botId)
  const [draft, setDraft] = useState<BotSettings | null>(null)

  if (tab && !TABS.some((t) => t.key === tab && t.key !== 'overview')) {
    return <Navigate to={`/app/bots/${botId}`} replace />
  }
  const active = (tab ?? 'overview') as TabKey

  if (bot.isLoading) {
    return (
      <>
        <BackLink />
        <div className="mb-6 flex items-center gap-4">
          <Skeleton className="size-[88px] rounded-3xl" />
          <div className="grid flex-1 gap-2">
            <Skeleton className="h-8 w-48" />
            <Skeleton className="h-5 w-24" />
          </div>
        </div>
        <Skeleton className="mb-6 h-11" />
        <div className="grid gap-4 lg:grid-cols-2">
          <Skeleton className="h-44" />
          <Skeleton className="h-44" />
        </div>
      </>
    )
  }

  if (bot.error instanceof ApiError && bot.error.status === 404) {
    return (
      <section className="grid items-center gap-6 rounded-3xl border border-paper-3 bg-cream p-6 sm:grid-cols-[auto_1fr] sm:p-8">
        <Pupil mood="surprised" size={128} className="mx-auto" />
        <div>
          <h1 className="font-display text-xl font-black text-ink">找不到这只小鱼</h1>
          <p className="mt-2 text-[15px] leading-relaxed text-ink-2">可能已经退学了，或者链接抄错了一位。回到小鱼列表看看吧。</p>
          <Link
            to="/app"
            className="mt-5 inline-flex h-10 items-center gap-2 rounded-xl border border-star-2 bg-star px-4 text-sm font-semibold text-ink shadow-[var(--shadow-star)] transition-[filter] duration-150 hover:brightness-[1.04]"
          >
            <ChevronLeft className="size-4" aria-hidden />
            回到我的小鱼
          </Link>
        </div>
      </section>
    )
  }

  if (bot.error || !bot.data) {
    return (
      <>
        <BackLink />
        <ErrorNote error={bot.error ?? new Error('没能读到这条小鱼')} onRetry={() => bot.refetch()} />
      </>
    )
  }

  const b = bot.data
  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(b.settings)

  return (
    <>
      <BackLink />
      <Header bot={b} />
      <Tabs botId={b.id} dirty={dirty} />
      {active === 'overview' && <Overview bot={b} />}
      {active === 'connect' && <Connect bot={b} />}
      {active === 'persona' && <Persona bot={b} draft={draft} setDraft={setDraft} />}
      {active === 'plugins' && <Plugins bot={b} />}
      {active === 'logs' && <Logs bot={b} />}
    </>
  )
}
