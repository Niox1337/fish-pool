import { useEffect, useState, type FormEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { QRCodeSVG } from 'qrcode.react'
import { CircleCheck, Copy, ExternalLink, QrCode, RotateCw, Smartphone, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { keys, useBotMutation } from '@/api/hooks'
import type { Bot, Platform, QqMode, QrTask } from '@/api/types'
import { Button, ConfirmDialog, Field, Input, Switch } from '@/components/ui'
import { useFieldId } from '@/lib/fieldId'
import { QqPenguin, WeChatBubble } from '@/components/icons'
import { cn } from '@/lib/cn'
import { PlatformStatusPill } from './status'
import { platformName } from './platform'
import { InlineName } from './shared'
import { errMsg, useErrorToast } from './errors'
import { copyText } from '../admin/copy'

type Kind = 'qq' | 'weixin'
type Phase = 'idle' | 'starting' | 'pending' | 'scanned' | 'expired' | 'error' | 'created'

function Countdown({ until }: { until: number }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [])
  const s = Math.max(0, Math.round((until - now) / 1000))
  return (
    <p className="mt-3 text-[13px] text-ink-3 tabular-nums">
      {Math.floor(s / 60)}:{String(s % 60).padStart(2, '0')} 后过期
    </p>
  )
}

function QrFlow({ botId, kind, again }: { botId: string; kind: Kind; again: boolean }) {
  const [phase, setPhase] = useState<Phase>('idle')
  const [task, setTask] = useState<QrTask | null>(null)
  const [message, setMessage] = useState('')
  const qc = useQueryClient()
  const live = phase === 'pending' || phase === 'scanned'
  const name = kind === 'qq' ? 'QQ ' : '微信'
  const mid = kind === 'qq' ? ' QQ ' : '微信'

  async function start() {
    setPhase('starting')
    setMessage('')
    try {
      const t = await api.qrStart(botId, kind)
      setTask(t)
      setPhase('pending')
    } catch (e) {
      setMessage(errMsg(e, '没能生成二维码，稍后再试'))
      setPhase('error')
    }
  }

  useEffect(() => {
    if (!task || !live) return
    let stop = false
    let timer: ReturnType<typeof setTimeout>
    const tick = async () => {
      if (stop) return
      if (Date.now() > task.expires_at) {
        setPhase('expired')
        return
      }
      try {
        const r = await api.qrPoll(botId, kind, task.task_id)
        if (stop) return
        if (r.status === 'created') {
          setPhase('created')
          toast.success(`${name}接好了${r.platform?.app_id ? `，AppID ${r.platform.app_id} 已自动填入` : ''}`)
          qc.invalidateQueries({ queryKey: keys.bot(botId) })
          qc.invalidateQueries({ queryKey: keys.bots })
          return
        }
        if (r.status === 'expired') return setPhase('expired')
        if (r.status === 'error') {
          setMessage(r.message ?? '扫码没有成功，重新生成一张试试')
          return setPhase('error')
        }
        if (r.status === 'scanned') setPhase('scanned')
      } catch (e) {
        if (stop) return
        setMessage(errMsg(e, '查询扫码结果失败'))
        return setPhase('error')
      }
      timer = setTimeout(tick, task.interval_s * 1000)
    }
    timer = setTimeout(tick, task.interval_s * 1000)
    return () => {
      stop = true
      clearTimeout(timer)
    }
  }, [task, live, botId, kind, name, qc])

  if (phase === 'idle' || phase === 'created' || (phase === 'starting' && !task)) {
    return (
      <div className="grid gap-3">
        {phase === 'created' && (
          <p className="inline-flex items-center gap-1.5 text-sm font-semibold text-ok" role="status">
            <CircleCheck className="size-4" aria-hidden />
            {name}已接入
          </p>
        )}
        <Button
          variant={again || phase === 'created' ? 'secondary' : 'primary'}
          onClick={start}
          loading={phase === 'starting'}
          className="justify-self-start"
        >
          <QrCode aria-hidden />
          {again || phase === 'created' ? '再扫一个' : `生成${mid}二维码`}
        </Button>
      </div>
    )
  }

  const dim = phase === 'expired' || phase === 'error' || phase === 'starting'
  return (
    <div className="flex flex-wrap items-start gap-5">
      <div className="relative shrink-0 rounded-2xl border border-paper-3 bg-white p-3">
        {task ? (
          <QRCodeSVG
            value={task.qr_url}
            size={176}
            marginSize={0}
            fgColor="#1b2163"
            title={`${name}扫码用的二维码`}
            className={cn(
              'block transition-[opacity,filter] duration-200',
              dim && 'opacity-25 blur-[2px]',
              phase === 'scanned' && 'opacity-20',
            )}
          />
        ) : (
          <div className="size-[176px]" />
        )}
        {phase === 'scanned' && (
          <div className="absolute inset-0 grid place-items-center p-4 text-center">
            <p className="rounded-xl bg-white/90 px-3 py-2 text-sm font-semibold text-ok">
              <CircleCheck className="mx-auto mb-1.5 size-8" aria-hidden />
              已扫码
              <br />
              请在手机上确认
            </p>
          </div>
        )}
        {(phase === 'expired' || phase === 'error') && (
          <div className="absolute inset-0 grid place-items-center p-4 text-center">
            <Button variant="primary" size="sm" onClick={start}>
              <RotateCw aria-hidden />
              重新生成
            </Button>
          </div>
        )}
      </div>
      <div className="min-w-0 flex-1 basis-52 text-sm leading-relaxed text-ink-2" aria-live="polite">
        {phase === 'pending' && (
          <>
            <p className="font-semibold text-ink">用手机{mid}扫一扫</p>
            <p className="mt-1">按手机上的提示确认。</p>
          </>
        )}
        {phase === 'scanned' && <p className="font-semibold text-ok">等待手机确认</p>}
        {phase === 'starting' && <p className="font-semibold text-ink">正在生成新的二维码…</p>}
        {phase === 'expired' && (
          <>
            <p className="font-semibold text-ink">二维码已过期</p>
          </>
        )}
        {phase === 'error' && (
          <>
            <p className="font-semibold text-bad">没有接上</p>
            <p className="mt-1">{message}</p>
          </>
        )}
        {live && task && <Countdown key={task.task_id} until={task.expires_at} />}
        {task && live && (
          <a
            href={task.qr_url}
            target="_blank"
            rel="noreferrer"
            className="mt-3 inline-flex items-center gap-1.5 text-[13px] font-semibold text-sky hover:underline"
          >
            <Smartphone className="size-3.5" aria-hidden />
            在本机打开
          </a>
        )}
      </div>
    </div>
  )
}

function QqManual({ botId }: { botId: string }) {
  const [appId, setAppId] = useState('')
  const [secret, setSecret] = useState('')
  const [error, setError] = useState<{ field: 'app' | 'secret' | 'form'; text: string } | null>(null)
  const add = useBotMutation(botId, (b: { app_id: string; secret: string }) => api.qqManual(botId, b))
  const appIdId = useFieldId('qq-appid')
  const secretId = useFieldId('qq-secret')

  function submit(e: FormEvent) {
    e.preventDefault()
    if (!/^\d{5,}$/.test(appId.trim())) return setError({ field: 'app', text: 'AppID 是一串数字，在 QQ 开放平台的开发设置里能看到' })
    if (!secret.trim()) return setError({ field: 'secret', text: '请填写 AppSecret' })
    setError(null)
    add.mutate(
      { app_id: appId.trim(), secret: secret.trim() },
      {
        onSuccess: () => {
          toast.success('QQ 机器人接好了')
          setAppId('')
          setSecret('')
        },
        onError: (err) => setError({ field: 'form', text: errMsg(err) }),
      },
    )
  }

  return (
    <details className="group mt-5 rounded-xl border border-paper-3 bg-paper/60">
      <summary className="cursor-pointer list-none rounded-xl px-4 py-3 text-sm font-semibold text-ink-2 hover:text-ink [&::-webkit-details-marker]:hidden">
        <span className="mr-1.5 inline-block transition-transform duration-150 group-open:rotate-90" aria-hidden>
          ›
        </span>
        手动填写 AppID 和密钥
      </summary>
      <form onSubmit={submit} className="grid gap-4 px-4 pt-1 pb-4" noValidate>
        <p className="text-[13px] text-ink-3">在 QQ 开放平台的机器人开发设置中查看凭证。</p>
        <Field label="AppID" htmlFor={appIdId} error={error?.field === 'app' ? error.text : undefined}>
          <Input
            id={appIdId}
            value={appId}
            onChange={(e) => setAppId(e.target.value)}
            inputMode="numeric"
            autoComplete="off"
            placeholder="102384756"
            aria-invalid={error?.field === 'app' || undefined}
          />
        </Field>
        <Field label="AppSecret" htmlFor={secretId} error={error && error.field !== 'app' ? error.text : undefined}>
          <Input
            id={secretId}
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            autoComplete="off"
            aria-invalid={error?.field === 'secret' || undefined}
          />
        </Field>
        <Button type="submit" variant="secondary" loading={add.isPending} className="justify-self-start">
          保存并接入
        </Button>
      </form>
    </details>
  )
}

function Channel({ icon, title, intro, children }: { icon: ReactNode; title: string; intro: ReactNode; children: ReactNode }) {
  return (
    <section className="rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <div className="mb-4 flex items-center gap-3">
        <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-sky-soft text-sky">{icon}</span>
        <h2 className="font-display text-lg font-black text-ink">{title}</h2>
      </div>
      <div className="mb-5 text-sm leading-relaxed text-ink-2">{intro}</div>
      {children}
    </section>
  )
}

const qqModes: { key: QqMode; label: string; note: string }[] = [
  { key: 'websocket', label: 'WebSocket（推荐）', note: '小鱼主动连接 QQ，不需要额外配置。' },
  { key: 'webhook', label: 'Webhook', note: 'QQ 把消息推送到托管班的回调地址，需要在 QQ 开放平台填写地址并勾选事件。' },
]

function QqConnection({ bot, p }: { bot: Bot; p: Platform }) {
  const showError = useErrorToast()
  const update = useBotMutation(bot.id, (b: { mode: QqMode }) => api.updatePlatform(bot.id, p.id, b))
  const mode: QqMode = update.isPending && update.variables ? update.variables.mode : (p.mode ?? 'websocket')
  const callback = `${window.location.origin}/api/qq/webhook/${p.id}`
  const current = qqModes.find((m) => m.key === mode)!

  function pick(next: QqMode) {
    if (next === mode || update.isPending) return
    update.mutate(
      { mode: next },
      {
        onSuccess: () =>
          toast.success(next === 'webhook' ? '已改为 Webhook，填好回调地址后就能收到消息' : '已改为 WebSocket，正在重新连接'),
        onError: (e) => showError(e),
      },
    )
  }

  return (
    <div className="mt-3 basis-full">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span id={`qq-mode-${p.id}`} className="text-[13px] font-semibold text-ink-2">
          连接方式
        </span>
        <div role="radiogroup" aria-labelledby={`qq-mode-${p.id}`} className="flex rounded-xl border border-paper-3 bg-paper-2 p-0.5">
          {qqModes.map((m) => (
            <button
              key={m.key}
              type="button"
              role="radio"
              aria-checked={mode === m.key}
              disabled={update.isPending}
              onClick={() => pick(m.key)}
              className={cn(
                'cursor-pointer rounded-[10px] px-3 py-1 text-[13px] font-semibold transition-colors duration-150 disabled:cursor-wait',
                mode === m.key ? 'bg-cream text-ink shadow-[0_1px_2px_rgb(27_33_99/0.12)]' : 'text-ink-3 hover:text-ink',
              )}
            >
              {m.label}
            </button>
          ))}
        </div>
      </div>
      <p className="mt-1.5 text-[13px] leading-relaxed text-ink-3">{current.note}</p>

      {mode === 'webhook' && (
        <div className="mt-3 rounded-xl border border-paper-3 bg-paper/60 p-4">
          <p className="text-[13px] font-semibold text-ink-2">回调地址</p>
          <div className="mt-1.5 flex flex-wrap items-center gap-2">
            <code className="min-w-0 flex-1 basis-60 break-all rounded-lg bg-white px-3 py-2 font-mono text-[13px] text-ink">
              {callback}
            </code>
            <Button size="sm" variant="secondary" onClick={() => copyText(callback, '回调地址')}>
              <Copy aria-hidden />
              复制
            </Button>
          </div>
          <ol className="mt-4 grid list-decimal gap-1.5 pl-5 text-[13px] leading-relaxed text-ink-2 marker:font-semibold marker:text-ink-3">
            <li>
              打开{' '}
              <a
                href="https://q.qq.com/qqbot/#/developer/webhook-setting"
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-0.5 font-semibold text-sky hover:underline"
              >
                QQ 开放平台的回调配置
                <ExternalLink className="size-3" aria-hidden />
              </a>
              ，选中 AppID 为 <span className="font-mono">{p.app_id}</span> 的机器人。
            </li>
            <li>粘贴回调地址并确认。验证通过后，这里的状态会变成「已连上」。</li>
            <li>
              在事件订阅里勾选群聊 @ 消息（<span className="font-mono">GROUP_AT_MESSAGE_CREATE</span>）和单聊消息（
              <span className="font-mono">C2C_MESSAGE_CREATE</span>）。
            </li>
          </ol>
          <p className="mt-3 text-[13px] leading-relaxed text-ink-3">以后改回 WebSocket 时，记得在开放平台删掉这个回调地址。</p>
        </div>
      )}
    </div>
  )
}

function PlatformRow({ bot, p }: { bot: Bot; p: Platform }) {
  const [confirm, setConfirm] = useState(false)
  const showError = useErrorToast()
  const update = useBotMutation(bot.id, (b: { enabled?: boolean; display_name?: string }) => api.updatePlatform(bot.id, p.id, b))
  const remove = useBotMutation(bot.id, () => api.deletePlatform(bot.id, p.id))
  const Icon = p.kind === 'qq_official' ? QqPenguin : WeChatBubble
  const label = p.display_name || platformName[p.kind]
  const enabled = update.isPending && update.variables?.enabled !== undefined ? update.variables.enabled : p.enabled

  return (
    <li className="flex flex-wrap items-start gap-x-4 gap-y-3 py-4 first:pt-0 last:pb-0">
      <Icon className="mt-1 size-6 shrink-0 text-sky" />
      <div className="min-w-0 flex-1 basis-56">
        <div className="flex flex-wrap items-center gap-2">
          <InlineName value={label} label="显示名称" onSave={(display_name) => update.mutateAsync({ display_name })}>
            <span className="truncate font-semibold text-ink">{label}</span>
          </InlineName>
          <PlatformStatusPill status={p.enabled ? p.status : 'disabled'} />
        </div>
        <dl className="mt-1 flex flex-wrap gap-x-4 gap-y-0.5 text-[13px] text-ink-3">
          <div className="flex gap-1">
            <dt>{p.kind === 'qq_official' ? 'AppID' : '账号'}</dt>
            <dd className="font-mono text-ink-2">{p.app_id}</dd>
          </div>
          <div className="flex gap-1">
            <dt>密钥</dt>
            <dd className="font-mono text-ink-2">{p.secret_masked}</dd>
          </div>
        </dl>
        {p.status_detail && (
          <p className={cn('mt-1.5 text-[13px] leading-relaxed', p.status === 'error' ? 'text-bad' : 'text-ink-2')}>{p.status_detail}</p>
        )}
        {p.kind === 'qq_official' && <QqConnection bot={bot} p={p} />}
      </div>
      <div className="flex items-center gap-3">
        <label className="flex items-center gap-2 text-[13px] font-semibold text-ink-2">
          <Switch
            checked={enabled}
            disabled={update.isPending}
            label={`启用 ${label}`}
            onCheckedChange={(v) =>
              update.mutate(
                { enabled: v },
                {
                  onSuccess: () => toast.success(v ? `${label} 已打开` : `${label} 已关闭，小鱼暂时不会在这里回话`),
                  onError: (e) => showError(e),
                },
              )
            }
          />
          {enabled ? '开' : '关'}
        </label>
        <Button size="sm" variant="ghost" onClick={() => setConfirm(true)} aria-label={`移除 ${label}`}>
          <Trash2 aria-hidden />
          移除
        </Button>
      </div>
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`移除 ${label}？`}
        body={<p>移除后会断开连接并删除凭证。要恢复，请重新接入。</p>}
        confirmLabel="确认移除"
        loading={remove.isPending}
        onConfirm={() =>
          remove.mutate(undefined, {
            onSuccess: () => {
              setConfirm(false)
              toast.success(`已移除 ${label}`)
            },
            onError: (e) => showError(e),
          })
        }
      />
    </li>
  )
}

export default function Connect({ bot }: { bot: Bot }) {
  const hasQq = bot.platforms.some((p) => p.kind === 'qq_official')
  const hasWx = bot.platforms.some((p) => p.kind === 'weixin_ilink')
  return (
    <>
      <div className="grid gap-4 lg:grid-cols-2">
        <Channel
          icon={<QqPenguin className="size-5" />}
          title="QQ 官方机器人"
          intro={<p>扫码创建机器人，自动填写 AppID 和密钥。默认用 WebSocket 连接，接入后可以在下方改成 Webhook。</p>}
        >
          <QrFlow botId={bot.id} kind="qq" again={hasQq} />
          <QqManual botId={bot.id} />
        </Channel>
        <Channel
          icon={<WeChatBubble className="size-5" />}
          title="微信"
          intro={<p>仅支持私聊。小鱼只能在对方最后一条消息后的 24 小时内回复。</p>}
        >
          <QrFlow botId={bot.id} kind="weixin" again={hasWx} />
        </Channel>
      </div>

      <section className="mt-8" aria-labelledby="platform-list">
        <h2 id="platform-list" className="mb-3 font-display text-lg font-black text-ink">
          已接入
          <span className="ml-2 text-sm font-semibold text-ink-3 tabular-nums">{bot.platforms.length}</span>
        </h2>
        {bot.platforms.length === 0 ? (
          <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-sm leading-relaxed text-ink-2">暂未接入平台。</p>
        ) : (
          <ul className="divide-y divide-paper-3 rounded-2xl border border-paper-3 bg-cream p-5">
            {bot.platforms.map((p) => (
              <PlatformRow key={p.id} bot={bot} p={p} />
            ))}
          </ul>
        )}
      </section>
    </>
  )
}
