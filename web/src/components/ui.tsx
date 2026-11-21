import { forwardRef, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode, type TextareaHTMLAttributes } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import * as SwitchPrimitive from '@radix-ui/react-switch'
import { LoaderCircle } from 'lucide-react'
import { cn } from '@/lib/cn'

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'cobalt'
type Size = 'sm' | 'md' | 'lg'

const variants: Record<Variant, string> = {
  primary:
    'bg-star text-ink border border-star-2 shadow-[inset_0_-2px_0_rgb(160_100_0/0.22),0_4px_10px_-6px_rgb(160_100_0/0.6)] hover:brightness-[1.04] active:translate-y-px',
  secondary: 'bg-cream text-ink border border-paper-3 hover:border-ink-3/50 hover:bg-white active:translate-y-px',
  ghost: 'text-ink-2 hover:bg-paper-2 hover:text-ink',
  danger: 'bg-cream text-bad border border-bad/30 hover:bg-bad-soft active:translate-y-px',
  cobalt: 'bg-cobalt text-white border border-cobalt-900 hover:bg-cobalt-700 active:translate-y-px',
}
const sizes: Record<Size, string> = {
  sm: 'h-8 px-3 text-[13px] gap-1.5 rounded-lg [&_svg]:size-3.5',
  md: 'h-10 px-4 text-sm gap-2 rounded-xl [&_svg]:size-4',
  lg: 'h-12 px-6 text-base gap-2 rounded-2xl [&_svg]:size-5',
}

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant
  size?: Size
  loading?: boolean
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'secondary', size = 'md', loading, className, children, disabled, type = 'button', ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={cn(
        'inline-flex shrink-0 cursor-pointer items-center justify-center whitespace-nowrap font-semibold transition-[background-color,border-color,filter,transform,color] duration-150 select-none',
        'disabled:cursor-not-allowed disabled:opacity-55 disabled:active:translate-y-0',
        variants[variant],
        sizes[size],
        className,
      )}
      {...rest}
    >
      {loading && <LoaderCircle className="animate-spin" aria-hidden />}
      {children}
    </button>
  )
})

const control =
  'w-full rounded-xl border border-paper-3 bg-white px-3.5 text-[15px] text-ink placeholder:text-ink-3 transition-[border-color,box-shadow] duration-150 hover:border-ink-3/45 focus:border-sky focus:shadow-[0_0_0_3px_rgb(47_91_216/0.18)] focus:outline-none disabled:bg-paper-2 disabled:text-ink-3 aria-invalid:border-bad aria-invalid:focus:shadow-[0_0_0_3px_rgb(196_61_75/0.16)]'

export const Input = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(function Input({ className, ...rest }, ref) {
  return <input ref={ref} className={cn(control, 'h-11', className)} {...rest} />
})

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(function Textarea(
  { className, ...rest },
  ref,
) {
  return <textarea ref={ref} className={cn(control, 'min-h-28 py-2.5 leading-relaxed', className)} {...rest} />
})

export function Field({
  label,
  hint,
  error,
  children,
  className,
  htmlFor,
  aside,
}: {
  label: ReactNode
  hint?: ReactNode
  error?: ReactNode
  children: ReactNode
  className?: string
  htmlFor?: string
  aside?: ReactNode
}) {
  return (
    <div className={cn('grid gap-1.5', className)}>
      <div className="flex items-baseline justify-between gap-3">
        <label htmlFor={htmlFor} className="text-sm font-semibold text-ink">
          {label}
        </label>
        {aside}
      </div>
      {children}
      {error ? (
        <p role="alert" className="text-[13px] text-bad">
          {error}
        </p>
      ) : hint ? (
        <p className="text-[13px] leading-relaxed text-ink-3">{hint}</p>
      ) : null}
    </div>
  )
}

export function Switch({
  checked,
  onCheckedChange,
  disabled,
  label,
  id,
}: {
  checked: boolean
  onCheckedChange: (v: boolean) => void
  disabled?: boolean
  label?: string
  id?: string
}) {
  return (
    <SwitchPrimitive.Root
      id={id}
      checked={checked}
      onCheckedChange={onCheckedChange}
      disabled={disabled}
      aria-label={label}
      className="relative h-6 w-11 shrink-0 cursor-pointer rounded-full bg-paper-3 transition-colors duration-200 data-[state=checked]:bg-ok disabled:cursor-not-allowed disabled:opacity-50"
    >
      <SwitchPrimitive.Thumb className="block size-5 translate-x-0.5 rounded-full bg-white shadow-[0_1px_3px_rgb(27_33_99/0.3)] transition-transform duration-200 ease-[var(--ease-out-expo)] data-[state=checked]:translate-x-[22px]" />
    </SwitchPrimitive.Root>
  )
}

export function PageHeader({
  title,
  description,
  actions,
  lead,
}: {
  title: ReactNode
  description?: ReactNode
  actions?: ReactNode
  lead?: ReactNode
}) {
  return (
    <header className="mb-6 flex flex-wrap items-center gap-4 sm:mb-8">
      {lead}
      <div className="min-w-0 flex-1">
        <h1 className="font-display text-2xl font-black text-ink sm:text-[28px]">{title}</h1>
        {description && <p className="mt-1 max-w-[60ch] text-[15px] leading-relaxed text-ink-2">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </header>
  )
}

type Tone = 'ok' | 'warn' | 'bad' | 'idle' | 'info'
const tones: Record<Tone, string> = {
  ok: 'bg-ok-soft text-ok',
  warn: 'bg-warn-soft text-warn',
  bad: 'bg-bad-soft text-bad',
  idle: 'bg-paper-2 text-ink-2',
  info: 'bg-sky-soft text-sky',
}

export function Pill({ tone, children, pulse, className }: { tone: Tone; children: ReactNode; pulse?: boolean; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-semibold whitespace-nowrap',
        tones[tone],
        className,
      )}
    >
      <span className={cn('size-1.5 rounded-full bg-current', pulse && 'animate-pulse')} aria-hidden />
      {children}
    </span>
  )
}

export function Skeleton({ className }: { className?: string }) {
  return <div className={cn('animate-pulse rounded-xl bg-paper-2', className)} aria-hidden />
}

export function ErrorNote({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const msg = error instanceof Error ? error.message : '出了点问题'
  return (
    <div role="alert" className="flex flex-wrap items-center gap-3 rounded-2xl border border-bad/25 bg-bad-soft px-4 py-3 text-sm text-bad">
      <span className="flex-1">{msg}</span>
      {onRetry && (
        <Button size="sm" variant="danger" onClick={onRetry}>
          再试一次
        </Button>
      )}
    </div>
  )
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  body,
  confirmLabel,
  onConfirm,
  loading,
  tone = 'danger',
  returnFocus,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
  title: ReactNode
  body: ReactNode
  confirmLabel: string
  onConfirm: () => void
  loading?: boolean
  tone?: 'danger' | 'primary'
  returnFocus?: () => void
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-cobalt-950/45 data-[state=open]:animate-[fade-in_150ms_ease-out]" />
        <Dialog.Content
          className="fixed top-1/2 left-1/2 z-50 w-[min(92vw,420px)] -translate-x-1/2 -translate-y-1/2 rounded-3xl border border-paper-3 bg-cream p-6 shadow-[var(--shadow-lift)] focus:outline-none motion-safe:data-[state=open]:animate-[pop-in_200ms_var(--ease-out-expo)]"
          onCloseAutoFocus={
            returnFocus
              ? (event) => {
                  event.preventDefault()
                  returnFocus()
                }
              : undefined
          }
        >
          <Dialog.Title className="font-display text-lg font-black wrap-anywhere text-ink">{title}</Dialog.Title>
          <Dialog.Description asChild>
            <div className="mt-2 text-[15px] leading-relaxed text-ink-2">{body}</div>
          </Dialog.Description>
          <div className="mt-6 flex justify-end gap-2">
            <Dialog.Close asChild>
              <Button variant="ghost">先不了</Button>
            </Dialog.Close>
            <Button variant={tone === 'danger' ? 'danger' : 'primary'} loading={loading} onClick={onConfirm}>
              {confirmLabel}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

export function Sheet({
  open,
  onOpenChange,
  title,
  description,
  children,
  side = 'bottom',
  returnFocus,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
  title: ReactNode
  description?: ReactNode
  children: ReactNode
  side?: 'bottom' | 'right'
  returnFocus?: () => void
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-cobalt-950/45 data-[state=open]:animate-[fade-in_150ms_ease-out]" />
        <Dialog.Content
          className={cn(
            'fixed z-50 bg-paper shadow-[var(--shadow-lift)] focus:outline-none',
            side === 'right'
              ? 'inset-y-0 right-0 flex w-full flex-col sm:max-w-xl motion-safe:data-[state=open]:animate-[slide-in-right_220ms_var(--ease-out-expo)]'
              : 'inset-x-0 bottom-0 max-h-[92dvh] overflow-y-auto rounded-t-3xl border border-paper-3 p-6 sm:inset-x-auto sm:top-1/2 sm:bottom-auto sm:left-1/2 sm:w-[min(92vw,520px)] sm:-translate-x-1/2 sm:-translate-y-1/2 sm:rounded-3xl data-[state=open]:animate-[pop-in_220ms_var(--ease-out-expo)]',
          )}
          onOpenAutoFocus={
            side === 'right'
              ? (event) => {
                  event.preventDefault()
                  if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus()
                }
              : undefined
          }
          onCloseAutoFocus={
            returnFocus
              ? (event) => {
                  event.preventDefault()
                  returnFocus()
                }
              : undefined
          }
        >
          <div className={side === 'right' ? 'border-b border-paper-3 bg-cream px-5 py-5 sm:px-8 sm:py-6' : undefined}>
            <Dialog.Title className="font-display text-xl font-black wrap-anywhere text-ink">{title}</Dialog.Title>
            <Dialog.Description className={description ? 'mt-1 text-sm leading-relaxed text-ink-2' : 'sr-only'}>
              {description ?? (typeof title === 'string' ? title : '')}
            </Dialog.Description>
          </div>
          <div className={side === 'right' ? 'min-h-0 flex-1' : 'mt-5'}>{children}</div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

export function Stat({ label, value, sub, tone }: { label: string; value: ReactNode; sub?: ReactNode; tone?: Tone }) {
  return (
    <div className="min-w-0">
      <dt className="text-[13px] text-ink-3">{label}</dt>
      <dd className={cn('mt-0.5 text-xl font-bold tabular-nums text-ink', tone === 'bad' && 'text-bad', tone === 'warn' && 'text-warn')}>
        {value}
      </dd>
      {sub && <dd className="text-[13px] text-ink-3 tabular-nums">{sub}</dd>}
    </div>
  )
}
