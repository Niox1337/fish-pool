import type { BotStatus, Platform, PluginStatus } from '@/api/types'
import { Pill } from '@/components/ui'
import { QqPenguin, WeChatBubble } from '@/components/icons'
import { cn } from '@/lib/cn'

const botStatus: Record<BotStatus, { label: string; tone: 'ok' | 'warn' | 'bad' | 'idle' }> = {
  running: { label: '运行中', tone: 'ok' },
  starting: { label: '正在入座', tone: 'warn' },
  stopped: { label: '在睡觉', tone: 'idle' },
  error: { label: '不舒服', tone: 'bad' },
}

export function BotStatusPill({ status }: { status: BotStatus }) {
  const s = botStatus[status]
  return (
    <Pill tone={s.tone} pulse={status === 'starting'}>
      {s.label}
    </Pill>
  )
}

const pluginStatus: Record<PluginStatus, { label: string; tone: 'ok' | 'warn' | 'bad' | 'idle' }> = {
  running: { label: '运行中', tone: 'ok' },
  installing: { label: '安装中', tone: 'warn' },
  stopped: { label: '已停用', tone: 'idle' },
  error: { label: '出错了', tone: 'bad' },
  over_quota: { label: '超出配额', tone: 'bad' },
}

export function PluginStatusPill({ status }: { status: PluginStatus }) {
  const s = pluginStatus[status]
  return (
    <Pill tone={s.tone} pulse={status === 'installing'}>
      {s.label}
    </Pill>
  )
}

export const platformName = { qq_official: 'QQ', weixin_ilink: '微信' } as const

export function PlatformBadge({ platform }: { platform: Platform }) {
  const Icon = platform.kind === 'qq_official' ? QqPenguin : WeChatBubble
  const bad = platform.status === 'error'
  const off = platform.status === 'disabled' || !platform.enabled
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-semibold',
        bad ? 'bg-bad-soft text-bad' : off ? 'bg-paper-2 text-ink-3' : 'bg-sky-soft text-sky',
      )}
      title={platform.status_detail}
    >
      <Icon className="size-3.5" />
      {platform.display_name || platformName[platform.kind]}
      {bad && '（出错）'}
      {off && !bad && '（已关）'}
    </span>
  )
}

const platformStatus: Record<Platform['status'], { label: string; tone: 'ok' | 'warn' | 'bad' | 'idle' }> = {
  connected: { label: '已连上', tone: 'ok' },
  connecting: { label: '连接中', tone: 'warn' },
  error: { label: '连不上', tone: 'bad' },
  disabled: { label: '已关闭', tone: 'idle' },
}

export function PlatformStatusPill({ status }: { status: Platform['status'] }) {
  const s = platformStatus[status]
  return (
    <Pill tone={s.tone} pulse={status === 'connecting'}>
      {s.label}
    </Pill>
  )
}
