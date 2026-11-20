import { Suspense, useEffect } from 'react'
import { Link, NavLink, Navigate, Outlet, useLocation, useNavigate } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Activity, Cpu, Gauge, KeyRound, LayoutDashboard, LogOut, Ticket, UserRound, Users, Fish } from 'lucide-react'
import { api } from '@/api/client'
import { keys, useMe } from '@/api/hooks'
import { cn } from '@/lib/cn'
import { daysLeft, fmtDate } from '@/lib/format'
import { Skeleton } from '@/components/ui'
import { ErrorBoundary } from '@/components/ErrorBoundary'
import logoMark from '@/assets/plates/logo-mark.webp'

const main = [
  { to: '/app', label: '我的小鱼', icon: Fish, end: true },
  { to: '/app/usage', label: '资源用量', icon: Gauge },
  { to: '/app/keys', label: '模型 Key', icon: KeyRound },
  { to: '/app/account', label: '账号', icon: UserRound },
]
const admin = [
  { to: '/app/admin', label: '总览', icon: LayoutDashboard, end: true },
  { to: '/app/admin/users', label: '用户', icon: Users },
  { to: '/app/admin/invites', label: '邀请码', icon: Ticket },
  { to: '/app/admin/system', label: '系统', icon: Activity },
]

function RailLink({ to, label, icon: Icon, end }: { to: string; label: string; icon: typeof Fish; end?: boolean }) {
  return (
    <NavLink
      to={to}
      end={end}
      className={({ isActive }) =>
        cn(
          'flex h-10 items-center gap-3 rounded-xl px-3 text-[15px] font-semibold transition-colors duration-150',
          isActive ? 'bg-cream text-ink shadow-[0_2px_0_rgb(0_0_0/0.12)]' : 'text-white/80 hover:bg-white/10 hover:text-white',
        )
      }
    >
      <Icon className="size-[18px] shrink-0" aria-hidden />
      {label}
    </NavLink>
  )
}

function TabLink({ to, label, icon: Icon, end }: { to: string; label: string; icon: typeof Fish; end?: boolean }) {
  return (
    <NavLink
      to={to}
      end={end}
      className={({ isActive }) =>
        cn(
          'flex min-w-0 flex-1 flex-col items-center gap-0.5 py-2 text-[11px] font-semibold',
          isActive ? 'text-star' : 'text-white/75',
        )
      }
    >
      <Icon className="size-5" aria-hidden />
      <span className="truncate">{label}</span>
    </NavLink>
  )
}

export default function ConsoleShell() {
  const { data: me, isLoading } = useMe()
  const loc = useLocation()
  const navigate = useNavigate()
  const qc = useQueryClient()
  const isAdmin = me?.role === 'admin'
  const inAdmin = loc.pathname.startsWith('/app/admin')

  useEffect(() => {
    window.scrollTo(0, 0)
  }, [loc.pathname])

  if (isLoading) {
    return (
      <div className="grid min-h-dvh place-items-center bg-paper">
        <Skeleton className="h-10 w-40" />
      </div>
    )
  }
  if (!me) return <Navigate to={`/login?next=${encodeURIComponent(loc.pathname)}`} replace />
  if (inAdmin && !isAdmin) return <Navigate to="/app" replace />

  async function logout() {
    try {
      await api.logout()
    } finally {
      qc.setQueryData(keys.me, null)
      qc.clear()
      navigate('/')
    }
  }

  const left = daysLeft(me.service_until)
  const serviceLine =
    me.role === 'admin'
      ? '管理员'
      : me.service_active
        ? `服务到 ${fmtDate(me.service_until)}`
        : '服务已到期'

  return (
    <div className="min-h-dvh bg-paper lg:grid lg:grid-cols-[240px_minmax(0,1fr)]">
      <aside className="sticky top-0 hidden h-dvh flex-col bg-cobalt px-3 py-4 text-white lg:flex">
        <Link to="/" className="mb-6 flex items-center gap-2.5 px-2">
          <img src={logoMark} alt="" className="h-9 w-auto" />
          <span className="font-display text-lg font-black">大肥鱼托管班</span>
        </Link>
        <nav aria-label="控制台" className="grid gap-1">
          {main.map((l) => (
            <RailLink key={l.to} {...l} />
          ))}
        </nav>
        {isAdmin && (
          <nav aria-label="管理" className="mt-6 grid gap-1">
            <p className="mb-1 flex items-center gap-2 px-3 text-xs font-semibold text-white/55">
              <Cpu className="size-3.5" aria-hidden />
              管理
            </p>
            {admin.map((l) => (
              <RailLink key={l.to} {...l} />
            ))}
          </nav>
        )}
        <div className="mt-auto rounded-2xl bg-cobalt-900/70 p-3">
          <p className="truncate text-sm font-semibold" title={me.email}>
            {me.email}
          </p>
          <p className={cn('mt-0.5 text-[13px] tabular-nums', me.service_active ? 'text-white/65' : 'text-pink')}>
            {serviceLine}
          </p>
          <button
            type="button"
            onClick={logout}
            className="mt-2 inline-flex cursor-pointer items-center gap-1.5 text-[13px] text-white/70 hover:text-white"
          >
            <LogOut className="size-3.5" aria-hidden />
            退出登录
          </button>
        </div>
      </aside>

      <header className="sticky top-0 z-30 flex h-14 items-center gap-2.5 bg-cobalt px-4 text-white lg:hidden">
        <Link to="/" className="flex items-center gap-2">
          <img src={logoMark} alt="" className="h-7 w-auto" />
          <span className="font-display text-base font-black">大肥鱼托管班</span>
        </Link>
        {isAdmin && (
          <Link
            to={inAdmin ? '/app' : '/app/admin'}
            className="ml-auto rounded-full bg-white/12 px-3 py-1 text-[13px] font-semibold text-white"
          >
            {inAdmin ? '回到我的小鱼' : '管理'}
          </Link>
        )}
      </header>

      <div className="min-w-0 pb-20 lg:pb-0">
        {!me.service_active && (
          <div role="status" className="border-b border-pink/50 bg-pink-soft">
            <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3 text-sm text-pink-ink sm:px-8">
              <span className="font-semibold">服务已到期，小鱼已停止。</span>
              <span>设置和插件已保留。</span>
              <Link to="/app/account#redeem" className="font-semibold underline underline-offset-2">
                去兑换
              </Link>
            </div>
          </div>
        )}
        {me.service_active && me.role !== 'admin' && left != null && left <= 7 && (
          <div role="status" className="border-b border-star/60 bg-warn-soft">
            <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3 text-sm text-warn sm:px-8">
              <span className="font-semibold tabular-nums">服务还剩 {left} 天</span>
              <span>到期后小鱼会停止运行。</span>
              <Link to="/app/account#redeem" className="font-semibold underline underline-offset-2">
                去兑换
              </Link>
            </div>
          </div>
        )}
        <main className="mx-auto max-w-6xl px-4 py-6 sm:px-8 sm:py-10">
          <ErrorBoundary variant="inline" resetKey={loc.pathname}>
            <Suspense
              fallback={
                <div className="grid gap-4">
                  <Skeleton className="h-9 w-48" />
                  <Skeleton className="h-40" />
                </div>
              }
            >
              <Outlet />
            </Suspense>
          </ErrorBoundary>
        </main>
      </div>

      <nav
        aria-label="控制台"
        className="fixed inset-x-0 bottom-0 z-30 flex bg-cobalt px-1 pb-[env(safe-area-inset-bottom)] lg:hidden"
      >
        {(inAdmin ? admin : main).map((l) => (
          <TabLink key={l.to} {...l} />
        ))}
      </nav>
    </div>
  )
}
