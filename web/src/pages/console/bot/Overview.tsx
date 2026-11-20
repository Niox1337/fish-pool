import { useState, type ReactNode } from 'react'
import { Link, useNavigate } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { ArrowRight, KeyRound, Link2, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { keys, useLlmKeys, useLogs, useMe, useUsage } from '@/api/hooks'
import type { Bot } from '@/api/types'
import { Button, ConfirmDialog, ErrorNote, Skeleton } from '@/components/ui'
import { FoodBar } from '@/components/FoodBar'
import { QqPenguin, WeChatBubble } from '@/components/icons'
import { cn } from '@/lib/cn'
import { platformName, PlatformStatusPill } from './status'
import { LogRow } from './Logs'
import { useErrorToast } from './errors'

function Panel({ title, action, children, className }: { title: string; action?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={cn('rounded-2xl border border-paper-3 bg-cream p-5', className)}>
      <div className="mb-4 flex items-baseline justify-between gap-3">
        <h2 className="font-display text-lg font-black text-ink">{title}</h2>
        {action}
      </div>
      {children}
    </section>
  )
}

function MoreLink({ to, children }: { to: string; children: ReactNode }) {
  return (
    <Link to={to} className="inline-flex items-center gap-1 text-sm font-semibold whitespace-nowrap text-sky hover:underline">
      {children}
      <ArrowRight className="size-3.5" aria-hidden />
    </Link>
  )
}

function Todo({ icon, title, body, children }: { icon: ReactNode; title: string; body: ReactNode; children: ReactNode }) {
  return (
    <li className="flex flex-wrap items-center gap-4 rounded-2xl border border-star/70 bg-warn-soft p-4 sm:p-5">
      <span className="grid size-11 shrink-0 place-items-center rounded-xl bg-cream text-warn">{icon}</span>
      <div className="min-w-0 flex-1 basis-56">
        <h3 className="font-semibold text-ink">{title}</h3>
        <p className="mt-0.5 text-sm leading-relaxed text-ink-2">{body}</p>
      </div>
      <div className="flex flex-wrap gap-2">{children}</div>
    </li>
  )
}

function ButtonLink({ to, children, primary }: { to: string; children: ReactNode; primary?: boolean }) {
  return (
    <Link
      to={to}
      className={cn(
        'inline-flex h-10 items-center gap-2 rounded-xl border px-4 text-sm font-semibold whitespace-nowrap transition-[background-color,border-color,filter] duration-150',
        primary
          ? 'border-star-2 bg-star text-ink shadow-[inset_0_-2px_0_rgb(160_100_0/0.22)] hover:brightness-[1.04]'
          : 'border-paper-3 bg-cream text-ink hover:border-ink-3/50 hover:bg-white',
      )}
    >
      {children}
    </Link>
  )
}

function Todos({ bot }: { bot: Bot }) {
  const llmKeys = useLlmKeys()
  const base = `/app/bots/${bot.id}`
  const noPlatform = bot.platforms.length === 0
  const noKey = !bot.settings.llm.key_id
  const haveKeys = (llmKeys.data?.length ?? 0) > 0
  if (!noPlatform && !noKey) return null
  return (
    <ul className="mb-6 grid gap-3" aria-label="还差的步骤">
      {noPlatform && (
        <Todo
          icon={<Link2 className="size-5" aria-hidden />}
          title="还没接入 QQ 或微信"
          body="接入以后，群友和好友才能找到这条小鱼。用手机扫个码就行，凭证会自动填好。"
        >
          <ButtonLink to={`${base}/connect`} primary>
            去接入
          </ButtonLink>
        </Todo>
      )}
      {noKey && (
        <Todo
          icon={<KeyRound className="size-5" aria-hidden />}
          title="还没选模型 Key"
          body={haveKeys ? '选一个 Key，小鱼才能开口回话。' : '小鱼要用你自己的模型 Key 才能说话。先添加一个，再回来给它选上。'}
        >
          {haveKeys || llmKeys.isLoading ? (
            <ButtonLink to={`${base}/persona`} primary>
              去选 Key
            </ButtonLink>
          ) : (
            <ButtonLink to="/app/keys" primary>
              添加模型 Key
            </ButtonLink>
          )}
        </Todo>
      )}
    </ul>
  )
}

function Platforms({ bot }: { bot: Bot }) {
  return (
    <Panel title="接入平台" action={<MoreLink to={`/app/bots/${bot.id}/connect`}>管理接入</MoreLink>}>
      {bot.platforms.length === 0 ? (
        <p className="text-sm leading-relaxed text-ink-3">还没有接入任何平台。</p>
      ) : (
        <ul className="grid gap-3">
          {bot.platforms.map((p) => {
            const Icon = p.kind === 'qq_official' ? QqPenguin : WeChatBubble
            return (
              <li key={p.id} className="flex items-start gap-3">
                <Icon className="mt-0.5 size-5 shrink-0 text-sky" />
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-semibold text-ink">{p.display_name || platformName[p.kind]}</span>
                    <PlatformStatusPill status={p.enabled ? p.status : 'disabled'} />
                  </div>
                  {p.status_detail && <p className="mt-0.5 text-[13px] text-ink-2">{p.status_detail}</p>}
                </div>
              </li>
            )
          })}
        </ul>
      )}
    </Panel>
  )
}

function Food({ bot }: { bot: Bot }) {
  const usage = useUsage()
  const MB = 1024 * 1024
  return (
    <Panel title="饭量" action={<MoreLink to="/app/usage">看明细</MoreLink>}>
      {usage.isLoading ? (
        <div className="grid gap-4">
          <Skeleton className="h-10" />
          <Skeleton className="h-10" />
        </div>
      ) : usage.error || !usage.data ? (
        <ErrorNote error={usage.error} onRetry={() => usage.refetch()} />
      ) : (
        (() => {
          const mine = usage.data.sources.filter((s) => s.bot_id === bot.id)
          const serv = (k: 'memory_bytes' | 'storage_bytes') =>
            mine.map((s) => ({ key: `${s.kind}-${s.plugin_id}`, label: s.label, bytes: s[k] }))
          return (
            <div className="grid gap-5">
              <FoodBar label="内存" capacity={usage.data.quota.memory_mb * MB} servings={serv('memory_bytes')} />
              <FoodBar label="存储" capacity={usage.data.quota.storage_mb * MB} servings={serv('storage_bytes')} compact />
              <p className="text-[13px] leading-relaxed text-ink-3">这里只算这条小鱼吃掉的部分，额度是你所有小鱼一起用的。</p>
            </div>
          )
        })()
      )}
    </Panel>
  )
}

function PluginHealth({ bot }: { bot: Bot }) {
  const { data: me } = useMe()
  const count = (f: (s: Bot['plugins'][number]['status']) => boolean) => bot.plugins.filter((p) => f(p.status)).length
  const rows = [
    { label: '运行中', n: count((s) => s === 'running'), tone: 'text-ok' },
    { label: '安装中', n: count((s) => s === 'installing'), tone: 'text-warn' },
    { label: '已停用', n: count((s) => s === 'stopped'), tone: 'text-ink-2' },
    { label: '需要看看', n: count((s) => s === 'error' || s === 'over_quota'), tone: 'text-bad' },
  ]
  return (
    <Panel title="插件" action={<MoreLink to={`/app/bots/${bot.id}/plugins`}>管理插件</MoreLink>}>
      {bot.plugins.length === 0 ? (
        <p className="text-sm leading-relaxed text-ink-3">还没装插件。插件能让小鱼多会几样本事，比如每日一言、表情包、查天气。</p>
      ) : (
        <dl className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          {rows.map((r) => (
            <div key={r.label}>
              <dt className="text-[13px] text-ink-3">{r.label}</dt>
              <dd className={cn('mt-0.5 text-2xl font-bold tabular-nums', r.n > 0 ? r.tone : 'text-ink-3')}>{r.n}</dd>
            </div>
          ))}
        </dl>
      )}
      {me && (
        <p className="mt-4 text-[13px] text-ink-3 tabular-nums">
          这条小鱼装了 {bot.plugins.length} 个，{me.quota.plugins == null ? '账号插件数量不限' : `账号额度共 ${me.quota.plugins} 个`}
        </p>
      )}
    </Panel>
  )
}

function RecentLogs({ bot }: { bot: Bot }) {
  const logs = useLogs(bot.id)
  const last = (logs.data?.items ?? []).slice(-5)
  return (
    <Panel title="最近日志" action={<MoreLink to={`/app/bots/${bot.id}/logs`}>看全部日志</MoreLink>}>
      {logs.isLoading ? (
        <Skeleton className="h-28" />
      ) : logs.error ? (
        <ErrorNote error={logs.error} onRetry={() => logs.refetch()} />
      ) : last.length === 0 ? (
        <p className="text-sm text-ink-3">小鱼启动以后，日志会出现在这里。</p>
      ) : (
        <ol className="overflow-hidden rounded-xl bg-cobalt-950 py-2 font-mono text-[13px] leading-relaxed">
          {last.map((l, i) => (
            <LogRow key={`${l.ts}-${i}`} line={l} stacked />
          ))}
        </ol>
      )}
    </Panel>
  )
}

function DeleteBot({ bot }: { bot: Bot }) {
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const qc = useQueryClient()
  const navigate = useNavigate()
  const showError = useErrorToast()

  async function remove() {
    setBusy(true)
    try {
      await api.deleteBot(bot.id)
      qc.removeQueries({ queryKey: keys.bot(bot.id) })
      qc.removeQueries({ queryKey: keys.logs(bot.id) })
      qc.invalidateQueries({ queryKey: keys.bots })
      qc.invalidateQueries({ queryKey: keys.usage })
      toast.success(`${bot.name} 已经退学了`)
      navigate('/app')
    } catch (e) {
      showError(e, '没能删除，稍后再试')
      setBusy(false)
    }
  }

  return (
    <section className="mt-10 flex flex-wrap items-center gap-4 border-t border-paper-3 pt-6" aria-labelledby="bot-delete">
      <div className="min-w-0 flex-1 basis-64">
        <h2 id="bot-delete" className="font-semibold text-ink">
          让这条小鱼退学
        </h2>
        <p className="mt-0.5 text-sm leading-relaxed text-ink-2">接入、人设、插件和聊天记录会一起删掉，没法找回。</p>
      </div>
      <Button variant="danger" onClick={() => setOpen(true)}>
        <Trash2 aria-hidden />
        删除小鱼
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={`删除 ${bot.name}？`}
        body={
          <p>
            它会马上停下，接入的 QQ 和微信会断开，装好的 {bot.plugins.length} 个插件和聊天记录都会删除，没法找回。
          </p>
        }
        confirmLabel="确认删除"
        loading={busy}
        onConfirm={remove}
      />
    </section>
  )
}

export default function Overview({ bot }: { bot: Bot }) {
  return (
    <>
      <Todos bot={bot} />
      <div className="grid gap-4 lg:grid-cols-2">
        <Platforms bot={bot} />
        <Food bot={bot} />
        <PluginHealth bot={bot} />
        <RecentLogs bot={bot} />
      </div>
      <DeleteBot bot={bot} />
    </>
  )
}
