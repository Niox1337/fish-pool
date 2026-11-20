import type { LogLine, Quota, User } from '@/api/types'
import { Field, Input, Pill, useFieldId } from '@/components/ui'
import { cn } from '@/lib/cn'
import { daysLeft, fmtDate, fmtTime } from '@/lib/format'

export const quotaFields: { key: keyof Quota; label: string; unit: string; min: number; max: number }[] = [
  { key: 'memory_mb', label: '内存', unit: 'MiB', min: 64, max: 65536 },
  { key: 'storage_mb', label: '存储', unit: 'MiB', min: 64, max: 1048576 },
  { key: 'bots', label: '小鱼', unit: '只', min: 0, max: 100 },
  { key: 'plugins', label: '插件', unit: '个', min: 0, max: 500 },
]

export type QuotaForm = Record<keyof Quota, string | null>

export function quotaToForm(quota: Quota) {
  return Object.fromEntries(quotaFields.map(({ key }) => [key, quota[key] == null ? null : String(quota[key])])) as QuotaForm
}

export function quotaFromForm(form: QuotaForm) {
  return Object.fromEntries(quotaFields.map(({ key }) => [key, form[key] == null ? null : Number(form[key])])) as Quota
}

export function quotaFieldError(raw: string | null, { min, max }: { min: number; max: number }) {
  if (raw === null && min === 0) return undefined
  const value = Number(raw)
  return raw?.trim() && Number.isInteger(value) && value >= min && value <= max ? undefined : `填写 ${min} 到 ${max} 的整数`
}

export function QuotaInputs({ form, onChange }: { form: QuotaForm; onChange: (key: keyof Quota, value: string | null) => void }) {
  const base = useFieldId('quota')
  return (
    <div className="grid grid-cols-2 gap-4">
      {quotaFields.map((f) => {
        const fieldId = `${base}-${f.key}`
        const error = quotaFieldError(form[f.key], f)
        return (
          <Field key={f.key} label={`${f.label}（${f.unit}）`} htmlFor={fieldId} error={error}>
            <Input id={fieldId} type="number" inputMode="numeric" min={f.min} max={f.max} disabled={form[f.key] === null}
              value={form[f.key] ?? ''} placeholder={form[f.key] === null ? '不限' : undefined}
              onChange={(e) => onChange(f.key, e.target.value)} aria-invalid={!!error} className="tabular-nums" />
            {f.min === 0 && (
              <label className="flex w-fit cursor-pointer items-center gap-2 text-sm text-ink-2">
                <input type="checkbox" className="accent-cobalt" aria-label={`${f.label}数量不限`} checked={form[f.key] === null}
                  onChange={(e) => onChange(f.key, e.target.checked ? null : '1')} />
                不限
              </label>
            )}
          </Field>
        )
      })}
    </div>
  )
}

export function ServiceStatus({ user, compact }: { user: Pick<User, 'role' | 'service_until' | 'service_active'>; compact?: boolean }) {
  if (user.role === 'admin') return <Pill tone="info">一直有效</Pill>
  const left = daysLeft(user.service_until)
  if (!user.service_until) return <Pill tone="idle">未开通</Pill>
  if (!user.service_active || left == null || left <= 0) {
    return (
      <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-0.5">
        <Pill tone="bad">已到期</Pill>
        {!compact && <span className="text-[13px] text-ink-3 tabular-nums">{fmtDate(user.service_until)}</span>}
      </span>
    )
  }
  return (
    <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-0.5">
      <Pill tone={left <= 7 ? 'warn' : 'ok'}>
        <span className="tabular-nums">剩 {left} 天</span>
      </Pill>
      {!compact && <span className="text-[13px] text-ink-3 tabular-nums">到 {fmtDate(user.service_until)}</span>}
    </span>
  )
}

const levelTone: Record<LogLine['level'], string> = {
  debug: 'text-ink-3',
  info: 'text-sky',
  warn: 'text-warn',
  error: 'text-bad',
}

export function LogList({ items }: { items: LogLine[] }) {
  return (
    <ol className="max-h-[60dvh] overflow-y-auto rounded-2xl border border-paper-3 bg-cream p-3 font-mono text-[12px] leading-relaxed sm:p-4 sm:text-[13px]" aria-live="polite">
      {items.map((l, i) => (
        <li key={`${l.ts}-${i}`} className={cn('grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 py-0.5', l.level === 'error' && 'rounded bg-bad-soft/60')}>
          <span className="text-ink-3 tabular-nums">{fmtTime(l.ts)}</span>
          <span className="break-words text-ink">
            <span className={cn('mr-2 font-semibold uppercase', levelTone[l.level])}>{l.level}</span>
            <span className="mr-2 text-ink-3">[{l.source}]</span>
            {l.message}
          </span>
        </li>
      ))}
    </ol>
  )
}
