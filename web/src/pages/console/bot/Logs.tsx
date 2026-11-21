import { useLayoutEffect, useMemo, useRef, useState } from 'react'
import { ArrowDown, Copy } from 'lucide-react'
import { toast } from 'sonner'
import { useLogs } from '@/api/hooks'
import type { Bot, LogLine } from '@/api/types'
import { Button, ErrorNote, Field, Skeleton } from '@/components/ui'
import { useFieldId } from '@/lib/fieldId'
import { cn } from '@/lib/cn'
import { fmtTime } from '@/lib/format'
import { Select } from './shared'

const LEVELS: LogLine['level'][] = ['debug', 'info', 'warn', 'error']
const levelStyle: Record<LogLine['level'], { label: string; cls: string }> = {
  debug: { label: 'DEBUG', cls: 'text-white/45' },
  info: { label: 'INFO', cls: 'text-[#9fb6ff]' },
  warn: { label: 'WARN', cls: 'text-star' },
  error: { label: 'ERROR', cls: 'text-[#ff9aa2]' },
}

const sourceLabel = (s: string) =>
  s === 'core'
    ? '核心'
    : s === 'qq_official'
      ? 'QQ'
      : s === 'weixin_ilink'
        ? '微信'
        : s === 'llm'
          ? '模型'
          : s.startsWith('plugin:')
            ? `插件 ${s.slice(7)}`
            : s

export function LogRow({ line, stacked }: { line: LogLine; stacked?: boolean }) {
  const lv = levelStyle[line.level] ?? levelStyle.info
  return (
    <li
      className={cn(
        'flex flex-wrap items-baseline gap-x-2.5 px-3 py-0.5 text-white/85',
        !stacked && 'sm:flex-nowrap',
        line.level === 'error' && 'bg-[#ff6b78]/10',
      )}
    >
      <time className="shrink-0 text-white/45 tabular-nums">{fmtTime(line.ts)}</time>
      <span className={cn('w-[5ch] shrink-0 font-semibold', lv.cls)}>{lv.label}</span>
      <span
        className={cn(
          'max-w-[16ch] shrink-0 truncate rounded bg-white/10 px-1.5 text-[12px] text-white/70',
          !stacked && 'sm:w-28 sm:max-w-28',
        )}
        title={line.source}
      >
        {sourceLabel(line.source)}
      </span>
      <span className={cn('min-w-0 basis-full break-words whitespace-pre-wrap', !stacked && 'sm:basis-auto')}>{line.message}</span>
    </li>
  )
}

const asText = (l: LogLine) => `${fmtTime(l.ts)} ${l.level.toUpperCase()} [${l.source}] ${l.message}`

export default function Logs({ bot }: { bot: Bot }) {
  const logs = useLogs(bot.id)
  const [source, setSource] = useState('all')
  const [minLevel, setMinLevel] = useState<LogLine['level']>('debug')
  const [stick, setStick] = useState(true)
  const box = useRef<HTMLOListElement>(null)
  const sourceId = useFieldId('log-source')
  const levelId = useFieldId('log-level')

  const items = useMemo(() => logs.data?.items ?? [], [logs.data])
  const sources = useMemo(() => {
    const set = new Set(['core', 'qq_official', 'weixin_ilink', 'llm', ...bot.plugins.map((p) => `plugin:${p.name}`)])
    for (const l of items) set.add(l.source)
    return [...set]
  }, [items, bot.plugins])
  const shown = useMemo(
    () => items.filter((l) => (source === 'all' || l.source === source) && LEVELS.indexOf(l.level) >= LEVELS.indexOf(minLevel)),
    [items, source, minLevel],
  )

  useLayoutEffect(() => {
    const el = box.current
    if (el && stick) el.scrollTop = el.scrollHeight
  }, [shown, stick])

  function onScroll() {
    const el = box.current
    if (!el) return
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24
    if (atBottom !== stick) setStick(atBottom)
  }

  async function copyAll() {
    try {
      await navigator.clipboard.writeText(shown.map(asText).join('\n'))
      toast.success(`复制了 ${shown.length} 行日志`)
    } catch {
      toast.error('浏览器没让复制，可以手动选中文字复制')
    }
  }

  return (
    <section aria-label="日志">
      <div className="mb-4 flex flex-wrap items-end gap-3">
        <Field label="来源" htmlFor={sourceId} className="min-w-0 flex-1 basis-40 sm:max-w-56">
          <Select
            id={sourceId}
            value={source}
            onChange={(e) => {
              setSource(e.target.value)
              setStick(true)
            }}
          >
            <option value="all">全部来源</option>
            {sources.map((s) => (
              <option key={s} value={s}>
                {sourceLabel(s)}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="级别" htmlFor={levelId} className="min-w-0 flex-1 basis-40 sm:max-w-56">
          <Select
            id={levelId}
            value={minLevel}
            onChange={(e) => {
              setMinLevel(e.target.value as LogLine['level'])
              setStick(true)
            }}
          >
            <option value="debug">全部级别</option>
            <option value="info">信息及以上</option>
            <option value="warn">警告及以上</option>
            <option value="error">只看错误</option>
          </Select>
        </Field>
        <Button onClick={copyAll} disabled={shown.length === 0} className="h-11 sm:ml-auto">
          <Copy aria-hidden />
          复制全部
        </Button>
      </div>

      {logs.isLoading ? (
        <Skeleton className="h-[60dvh] min-h-72" />
      ) : logs.error ? (
        <ErrorNote error={logs.error} onRetry={() => logs.refetch()} />
      ) : items.length === 0 ? (
        <div className="grid min-h-60 place-items-center rounded-2xl border border-dashed border-paper-3 p-6 text-center">
          <div>
            <p className="font-semibold text-ink">还没有日志</p>
            <p className="mt-1 text-sm leading-relaxed text-ink-2">{bot.status === 'stopped' ? '启动小鱼后查看日志。' : '每 4 秒刷新。'}</p>
          </div>
        </div>
      ) : (
        <div className="relative">
          <ol
            ref={box}
            onScroll={onScroll}
            tabIndex={0}
            aria-label="日志内容"
            aria-live="off"
            className="h-[60dvh] min-h-72 overflow-y-auto rounded-2xl bg-cobalt-950 py-2 font-mono text-[13px] leading-relaxed focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-sky"
          >
            {shown.length === 0 ? (
              <li className="px-3 py-2 text-white/60">没有匹配的日志。</li>
            ) : (
              shown.map((l, i) => <LogRow key={`${l.ts}-${i}`} line={l} />)
            )}
          </ol>
          {!stick && (
            <Button
              size="sm"
              variant="primary"
              onClick={() => setStick(true)}
              className="absolute right-4 bottom-4 animate-[fade-in_150ms_ease-out]"
            >
              <ArrowDown aria-hidden />
              跳到最新
            </Button>
          )}
        </div>
      )}
      {items.length > 0 && (
        <p className="mt-3 text-[13px] text-ink-3 tabular-nums">
          {shown.length !== items.length ? `${shown.length} / ${items.length}` : items.length} 行 · {stick ? '自动跟随' : '已暂停跟随'}
        </p>
      )}
    </section>
  )
}
