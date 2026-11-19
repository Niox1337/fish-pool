import { useState, type KeyboardEvent, type ReactNode, type SelectHTMLAttributes } from 'react'
import { Link } from 'react-router'
import { Check, ChevronDown, Pencil, X } from 'lucide-react'
import { errMsg, isExpired, useErrorToast } from './errors'
import { Button, Field, Input, Switch } from '@/components/ui'
import { cn } from '@/lib/cn'

export function ActionError({ error, onDismiss }: { error: unknown; onDismiss?: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-2xl border border-bad/25 bg-bad-soft px-4 py-3 text-sm text-bad">
      <span className="min-w-0 flex-1">{errMsg(error)}</span>
      {isExpired(error) && (
        <Link to="/app/account#redeem" className="font-semibold underline underline-offset-2">
          去兑换邀请码
        </Link>
      )}
      {onDismiss && (
        <button type="button" onClick={onDismiss} aria-label="关闭提示" className="cursor-pointer rounded-lg p-1 hover:bg-bad/10">
          <X className="size-4" aria-hidden />
        </button>
      )}
    </div>
  )
}

const selectControl =
  'h-11 w-full cursor-pointer appearance-none rounded-xl border border-paper-3 bg-white pr-10 pl-3.5 text-[15px] text-ink transition-[border-color,box-shadow] duration-150 hover:border-ink-3/45 focus:border-sky focus:shadow-[0_0_0_3px_rgb(47_91_216/0.18)] focus:outline-none disabled:cursor-not-allowed disabled:bg-paper-2 disabled:text-ink-3'

export function Select({ className, children, ...rest }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <div className={cn('relative', className)}>
      <select className={selectControl} {...rest}>
        {children}
      </select>
      <ChevronDown className="pointer-events-none absolute top-1/2 right-3.5 size-4 -translate-y-1/2 text-ink-3" aria-hidden />
    </div>
  )
}

export function ChipsInput({
  id,
  value,
  onChange,
  placeholder,
  label,
}: {
  id?: string
  value: string[]
  onChange: (v: string[]) => void
  placeholder?: string
  label: string
}) {
  const [text, setText] = useState('')

  function commit(raw = text) {
    const parts = raw
      .split(/[,，\n]/)
      .map((s) => s.trim())
      .filter(Boolean)
    if (parts.length) onChange([...value, ...parts.filter((p) => !value.includes(p))])
    setText('')
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault()
      commit()
    } else if (e.key === 'Backspace' && !text && value.length) {
      onChange(value.slice(0, -1))
    }
  }

  return (
    <div className="flex min-h-11 flex-wrap items-center gap-1.5 rounded-xl border border-paper-3 bg-white px-2 py-1.5 transition-[border-color,box-shadow] duration-150 focus-within:border-sky focus-within:shadow-[0_0_0_3px_rgb(47_91_216/0.18)] hover:border-ink-3/45">
      {value.map((v) => (
        <span key={v} className="inline-flex max-w-full items-center gap-1 rounded-lg bg-sky-soft py-0.5 pr-1 pl-2 text-[13px] font-semibold text-sky">
          <span className="truncate">{v}</span>
          <button
            type="button"
            onClick={() => onChange(value.filter((x) => x !== v))}
            aria-label={`从${label}里移除 ${v}`}
            className="cursor-pointer rounded p-0.5 hover:bg-sky/15"
          >
            <X className="size-3" aria-hidden />
          </button>
        </span>
      ))}
      <input
        id={id}
        value={text}
        onChange={(e) => {
          const v = e.target.value
          if (/[,，]/.test(v)) commit(v)
          else setText(v)
        }}
        onKeyDown={onKey}
        onBlur={() => commit()}
        placeholder={value.length ? '' : placeholder}
        aria-label={id ? undefined : label}
        className="h-8 min-w-[8ch] flex-1 bg-transparent px-1.5 text-[15px] text-ink placeholder:text-ink-3 focus:outline-none"
      />
    </div>
  )
}

export function RangeField({
  id,
  label,
  hint,
  value,
  onChange,
  min,
  max,
  step = 1,
  format = (v) => String(v),
}: {
  id: string
  label: ReactNode
  hint?: ReactNode
  value: number
  onChange: (v: number) => void
  min: number
  max: number
  step?: number
  format?: (v: number) => string
}) {
  return (
    <Field
      label={label}
      htmlFor={id}
      hint={hint}
      aside={
        <output htmlFor={id} className="rounded-lg bg-paper-2 px-2 py-0.5 text-sm font-bold tabular-nums text-ink">
          {format(value)}
        </output>
      }
    >
      <div className="flex items-center gap-3">
        <span className="text-[13px] text-ink-3 tabular-nums">{format(min)}</span>
        <input
          id={id}
          type="range"
          min={min}
          max={max}
          step={step}
          value={value}
          aria-valuetext={format(value)}
          onChange={(e) => onChange(Number(e.target.value))}
          className="h-2 w-full min-w-0 cursor-pointer accent-cobalt"
        />
        <span className="text-[13px] text-ink-3 tabular-nums">{format(max)}</span>
      </div>
    </Field>
  )
}

export function SwitchRow({
  label,
  hint,
  checked,
  onChange,
  disabled,
}: {
  label: string
  hint?: ReactNode
  checked: boolean
  onChange: (v: boolean) => void
  disabled?: boolean
}) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="min-w-0">
        <p className="text-sm font-semibold text-ink">{label}</p>
        {hint && <p className="mt-0.5 text-[13px] leading-relaxed text-ink-3">{hint}</p>}
      </div>
      <Switch checked={checked} onCheckedChange={onChange} label={label} disabled={disabled} />
    </div>
  )
}

export function InlineName({
  value,
  onSave,
  label,
  maxLength = 24,
  children,
}: {
  value: string
  onSave: (v: string) => Promise<unknown>
  label: string
  maxLength?: number
  children: ReactNode
}) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(value)
  const [busy, setBusy] = useState(false)
  const showError = useErrorToast()

  async function save() {
    const v = draft.trim()
    if (!v || v === value) {
      setEditing(false)
      return
    }
    setBusy(true)
    try {
      await onSave(v)
      setEditing(false)
    } catch (e) {
      showError(e, '没能改名，稍后再试')
    } finally {
      setBusy(false)
    }
  }

  if (!editing) {
    return (
      <span className="inline-flex min-w-0 items-center gap-1.5">
        {children}
        <button
          type="button"
          onClick={() => {
            setDraft(value)
            setEditing(true)
          }}
          aria-label={`修改${label}`}
          title={`修改${label}`}
          className="shrink-0 cursor-pointer rounded-lg p-1.5 text-ink-3 transition-colors duration-150 hover:bg-paper-2 hover:text-ink"
        >
          <Pencil className="size-4" aria-hidden />
        </button>
      </span>
    )
  }

  return (
    <form
      className="flex min-w-0 items-center gap-1.5"
      onSubmit={(e) => {
        e.preventDefault()
        save()
      }}
    >
      <Input
        autoFocus
        value={draft}
        maxLength={maxLength}
        aria-label={label}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => e.key === 'Escape' && setEditing(false)}
        className="h-10 w-auto min-w-0 flex-1 sm:w-64 sm:flex-none"
      />
      <Button type="submit" size="sm" variant="primary" loading={busy} aria-label="保存名字" className="h-10 w-10 px-0">
        {!busy && <Check aria-hidden />}
      </Button>
      <Button size="sm" variant="ghost" onClick={() => setEditing(false)} aria-label="取消修改" className="h-10 w-10 px-0">
        <X aria-hidden />
      </Button>
    </form>
  )
}
