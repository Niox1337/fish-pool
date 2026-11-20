import { Component, type ErrorInfo, type ReactNode } from 'react'
import { Button } from '@/components/ui'
import { Pupil } from '@/components/Pupil'
import { cn } from '@/lib/cn'

const RELOAD_KEY = 'fish:chunk-reload-at'
const RELOAD_GUARD_MS = 30_000

/** True for errors raised when a lazily loaded page script cannot be fetched or parsed. */
export function isChunkError(error: unknown): boolean {
  const text = error instanceof Error ? `${error.name} ${error.message}` : String(error)
  return /dynamically imported module|Importing a module script failed|module script|ChunkLoadError|Unable to preload/i.test(text)
}

/**
 * Reloads the page so it picks up the current deployment.
 * Returns false without reloading when a reload already happened in the last 30 s or storage is unavailable.
 */
export function reloadForNewVersion(): boolean {
  try {
    const last = Number(sessionStorage.getItem(RELOAD_KEY) || 0)
    if (Date.now() - last < RELOAD_GUARD_MS) return false
    sessionStorage.setItem(RELOAD_KEY, String(Date.now()))
  } catch {
    return false
  }
  window.location.reload()
  return true
}

type Props = {
  children: ReactNode
  /** The boundary clears its error when this value changes, for example on navigation. */
  resetKey?: string
  /** `screen` fills the viewport, `inline` sits inside the console layout. */
  variant?: 'screen' | 'inline'
}

type State = { error: unknown; reloading: boolean }

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, reloading: false }

  static getDerivedStateFromError(error: unknown): Partial<State> {
    return { error: error ?? new Error('unknown error') }
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error(error, info.componentStack)
    if (isChunkError(error) && reloadForNewVersion()) this.setState({ reloading: true })
  }

  componentDidUpdate(prev: Props) {
    if (this.state.error && !this.state.reloading && prev.resetKey !== this.props.resetKey) {
      this.setState({ error: null })
    }
  }

  render() {
    const { error, reloading } = this.state
    if (!error) return this.props.children
    const chunk = isChunkError(error)
    const screen = this.props.variant !== 'inline'
    return (
      <div
        role="alert"
        className={cn('grid place-items-center px-6 text-center', screen ? 'min-h-dvh bg-paper' : 'py-16')}
      >
        <div className="flex max-w-sm flex-col items-center gap-4">
          <Pupil mood={reloading ? 'eat' : 'dizzy'} size={112} />
          <h1 className="font-display text-xl font-bold text-ink">
            {reloading ? '正在换上新版本' : chunk ? '网站刚刚更新过' : '这一页出了点问题'}
          </h1>
          {!reloading && (
            <>
              <p className="text-[15px] text-ink-2">
                {chunk ? '刷新一下就能用新版本继续了。' : '刷新页面通常就能恢复。如果反复出现，请告诉管理员。'}
              </p>
              <Button variant="primary" onClick={() => window.location.reload()}>
                刷新页面
              </Button>
            </>
          )}
        </div>
      </div>
    )
  }
}
