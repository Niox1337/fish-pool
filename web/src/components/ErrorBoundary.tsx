import { Component, type ErrorInfo, type ReactNode } from 'react'
import { Button } from '@/components/ui'
import { Pupil } from '@/components/Pupil'
import { cn } from '@/lib/cn'
import { isChunkError, reloadForNewVersion } from '@/lib/reload'

type Props = {
  children: ReactNode
  /** The boundary clears its error when this value changes, for example on navigation. */
  resetKey?: string
  /** `screen` fills the viewport, `inline` sits inside the console layout. */
  variant?: 'screen' | 'inline'
}

type State = { error: unknown; reloading: boolean; resetKey?: string }

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, reloading: false, resetKey: this.props.resetKey }

  static getDerivedStateFromError(error: unknown): Partial<State> {
    return { error: error ?? new Error('unknown error') }
  }

  static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
    if (props.resetKey === state.resetKey) return null
    if (state.error && !state.reloading) return { error: null, resetKey: props.resetKey }
    return { resetKey: props.resetKey }
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error(error, info.componentStack)
    if (isChunkError(error) && reloadForNewVersion()) this.setState({ reloading: true })
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
