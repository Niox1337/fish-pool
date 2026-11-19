import { cn } from '@/lib/cn'
import { fmtBytes, pct } from '@/lib/format'

export type Serving = { key: string; label: string; bytes: number }

const palette = ['bg-cobalt', 'bg-sky', 'bg-pink', 'bg-star', 'bg-[#8fb3ff]', 'bg-[#e58aa5]', 'bg-[#f2a93b]', 'bg-[#6c7fd8]']

export function servingColor(i: number) {
  return palette[i % palette.length]
}

export function FoodBar({
  servings,
  capacity,
  label,
  compact,
}: {
  servings: Serving[]
  capacity: number
  label: string
  compact?: boolean
}) {
  const used = servings.reduce((a, s) => a + s.bytes, 0)
  const over = used > capacity
  const free = Math.max(0, capacity - used)
  return (
    <figure className="grid gap-2">
      <figcaption className="flex items-baseline justify-between gap-3 text-sm">
        <span className="font-semibold text-ink">{label}</span>
        <span className={cn('tabular-nums text-ink-2', over && 'font-semibold text-bad')}>
          {fmtBytes(used)} / {fmtBytes(capacity)}
          {!compact && <span className="ml-2 text-ink-3">{Math.round(pct(used, capacity))}%</span>}
        </span>
      </figcaption>
      <div
        className={cn('flex h-3.5 gap-[3px] overflow-hidden rounded-full bg-paper-2', over && 'ring-2 ring-bad/40')}
        role="img"
        aria-label={`${label}：已用 ${fmtBytes(used)}，共 ${fmtBytes(capacity)}`}
      >
        {servings.map((s, i) =>
          s.bytes > 0 ? (
            <span
              key={s.key}
              title={`${s.label} ${fmtBytes(s.bytes)}`}
              className={cn('h-full min-w-[3px] transition-[flex-grow] duration-500', servingColor(i))}
              style={{ flexGrow: s.bytes, flexBasis: 0 }}
            />
          ) : null,
        )}
        {free > 0 && <span className="h-full" style={{ flexGrow: free, flexBasis: 0 }} />}
      </div>
      {!compact && (
        <ul className="flex flex-wrap gap-x-4 gap-y-1.5 text-[13px] text-ink-2">
          {servings.map((s, i) => (
            <li key={s.key} className="inline-flex items-center gap-1.5">
              <i className={cn('size-2.5 rounded-[3px]', servingColor(i))} aria-hidden />
              {s.label}
              <span className="tabular-nums text-ink-3">{fmtBytes(s.bytes)}</span>
            </li>
          ))}
        </ul>
      )}
    </figure>
  )
}
