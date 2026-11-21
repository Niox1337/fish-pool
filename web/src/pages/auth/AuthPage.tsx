import { useEffect, useState, type FormEvent } from 'react'
import { Link, Navigate, useNavigate, useSearchParams } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { api, ApiError } from '@/api/client'
import { keys, useMe } from '@/api/hooks'
import { Button, Field, Input } from '@/components/ui'
import { Pupil } from '@/components/Pupil'
import illustration from '@/assets/plates/illustration.webp'
import logoMark from '@/assets/plates/logo-mark.webp'

type Mode = 'login' | 'register' | 'reset'

const titles: Record<Mode, { h: string; sub: string }> = {
  login: { h: '欢迎回来', sub: '登录后管理你托管的小鱼。' },
  register: { h: '开通托管', sub: '使用邀请码和邮箱注册。' },
  reset: { h: '找回密码', sub: '往你的邮箱发一个验证码，然后设置新密码。' },
}

function normInvite(raw: string): string {
  const s = raw
    .toUpperCase()
    .replace(/[^0-9A-Z]/g, '')
    .replace(/^FISH/, '')
  const body = s.slice(0, 8)
  if (!body) return raw.toUpperCase().startsWith('F') ? raw.toUpperCase().slice(0, 5) : ''
  return `FISH-${body.slice(0, 4)}${body.length > 4 ? '-' + body.slice(4) : ''}`
}

function Captcha({
  value,
  onChange,
  onId,
  bump,
}: {
  value: string
  onChange: (v: string) => void
  onId: (id: string) => void
  bump: number
}) {
  const [n, setN] = useState(0)
  const request = `${n}:${bump}`
  const [loaded, setLoaded] = useState<{ request: string; svg: string } | null>(null)
  const svg = loaded?.request === request ? loaded.svg : null
  useEffect(() => {
    let live = true
    api
      .captcha()
      .then((c) => {
        if (!live) return
        setLoaded({ request, svg: c.svg })
        onId(c.id)
      })
      .catch(() => live && setLoaded({ request, svg: '' }))
    return () => {
      live = false
    }
  }, [request, onId])
  return (
    <Field label="图形验证码" htmlFor="captcha">
      <div className="flex gap-2">
        <Input
          id="captcha"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          autoComplete="off"
          inputMode="text"
          placeholder="看图填字"
          required
          className="flex-1"
        />
        <button
          type="button"
          onClick={() => {
            onChange('')
            setN((x) => x + 1)
          }}
          className="group relative h-11 w-[132px] shrink-0 cursor-pointer overflow-hidden rounded-xl border border-paper-3 bg-white"
          aria-label="换一张验证码"
          title="看不清，换一张"
        >
          {svg ? (
            <span className="block h-full w-full [&>svg]:h-full [&>svg]:w-full" dangerouslySetInnerHTML={{ __html: svg }} />
          ) : (
            <span className="block h-full w-full animate-pulse bg-paper-2" />
          )}
          <RefreshCw
            className="absolute right-1 bottom-1 size-3.5 text-ink-3 opacity-0 transition-opacity group-hover:opacity-100"
            aria-hidden
          />
        </button>
      </div>
    </Field>
  )
}

function useCooldown() {
  const [left, setLeft] = useState(0)
  useEffect(() => {
    if (left <= 0) return
    const t = setTimeout(() => setLeft((x) => x - 1), 1000)
    return () => clearTimeout(t)
  }, [left])
  return [left, setLeft] as const
}

export default function AuthPage({ mode }: { mode: Mode }) {
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const qc = useQueryClient()
  const { data: me } = useMe()

  const [email, setEmail] = useState(params.get('email') ?? '')
  const [password, setPassword] = useState('')
  const [invite, setInvite] = useState(normInvite(params.get('invite') ?? ''))
  const [code, setCode] = useState('')
  const [captchaId, setCaptchaId] = useState('')
  const [captcha, setCaptcha] = useState('')
  const [captchaBump, setCaptchaBump] = useState(0)
  const [needCaptcha, setNeedCaptcha] = useState(mode !== 'login')
  const [agree, setAgree] = useState(false)
  const [sending, setSending] = useState(false)
  const [sent, setSent] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<{ field?: string; msg: string } | null>(null)
  const [cooldown, setCooldown] = useCooldown()

  const next = params.get('next')
  const t = titles[mode]

  const [shownMode, setShownMode] = useState(mode)
  if (shownMode !== mode) {
    setShownMode(mode)
    setError(null)
    setNeedCaptcha(mode !== 'login')
  }

  if (me && mode !== 'reset') return <Navigate to={next || '/app'} replace />

  function fail(e: unknown) {
    if (e instanceof ApiError) {
      const field = e.code.startsWith('invite')
        ? 'invite'
        : e.code.startsWith('captcha')
          ? 'captcha'
          : e.code.startsWith('code')
            ? 'code'
            : undefined
      if (e.code === 'captcha_required') setNeedCaptcha(true)
      if (field === 'captcha' || e.code === 'captcha_required') {
        setCaptcha('')
        setCaptchaBump((x) => x + 1)
      }
      setError({ field, msg: e.message })
    } else setError({ msg: '出了点问题，稍后再试' })
  }

  async function sendCode() {
    setError(null)
    if (!email) return setError({ field: 'email', msg: '先填邮箱' })
    if (mode === 'register' && !/^FISH-[0-9A-Z]{4}-[0-9A-Z]{4}$/.test(invite))
      return setError({ field: 'invite', msg: '邀请码的格式是 FISH-XXXX-XXXX' })
    if (!captcha) return setError({ field: 'captcha', msg: '先填图形验证码' })
    setSending(true)
    try {
      const r = await api.sendCode({
        email,
        purpose: mode === 'register' ? 'register' : 'reset',
        captcha_id: captchaId,
        captcha_answer: captcha,
        invite_code: mode === 'register' ? invite : undefined,
      })
      setSent(true)
      setCooldown(r.cooldown_s)
    } catch (e) {
      fail(e)
    } finally {
      setSending(false)
    }
  }

  async function submit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    setBusy(true)
    try {
      if (mode === 'login') {
        const r = await api.login({
          email,
          password,
          ...(needCaptcha ? { captcha_id: captchaId, captcha_answer: captcha } : {}),
        })
        qc.setQueryData(keys.me, r.user)
        navigate(next || '/app', { replace: true })
      } else if (mode === 'register') {
        if (!agree) {
          setError({ field: 'agree', msg: '需要先同意托管须知' })
          return
        }
        const r = await api.register({ email, password, code, invite_code: invite, agree: true })
        qc.setQueryData(keys.me, r.user)
        navigate('/app', { replace: true })
      } else {
        await api.reset({ email, code, password })
        navigate(`/login?email=${encodeURIComponent(email)}`, { replace: true })
      }
    } catch (err) {
      fail(err)
    } finally {
      setBusy(false)
    }
  }

  const errFor = (f: string) => (error?.field === f ? error.msg : undefined)

  return (
    <div className="min-h-dvh bg-cobalt lg:grid lg:grid-cols-[minmax(0,1.1fr)_minmax(440px,1fr)]">
      <aside className="relative hidden overflow-hidden lg:flex lg:flex-col lg:justify-between lg:p-10">
        <Link to="/" className="flex items-center gap-3 text-white">
          <img src={logoMark} alt="" className="h-11 w-auto" />
          <span className="font-display text-2xl font-black">大肥鱼托管班</span>
        </Link>
        <img
          src={illustration}
          alt="蓝发小老师在阅读角给四条大肥鱼小朋友讲绘本"
          className="mx-auto w-full max-w-[640px] rounded-[28px] shadow-[0_30px_60px_-30px_rgb(0_0_0/0.6)]"
        />
        <p className="text-sm text-white/70">QQ 扫码就能接入，微信也行。自带模型 Key，装上 AstrBot 插件。</p>
      </aside>

      <main className="flex min-h-dvh flex-col bg-paper px-4 py-6 sm:px-10 lg:rounded-l-[36px] lg:py-10">
        <Link to="/" className="flex items-center gap-2 lg:hidden">
          <img src={logoMark} alt="" className="h-8 w-auto" />
          <span className="font-display text-lg font-black text-ink">大肥鱼托管班</span>
        </Link>
        <div className="m-auto w-full max-w-[400px] py-8">
          <div className="mb-7 flex items-end gap-3">
            <Pupil mood={mode === 'register' ? 'happy' : mode === 'reset' ? 'surprised' : 'idle'} size={76} />
            <div>
              <h1 className="font-display text-[28px] font-black leading-tight text-ink">{t.h}</h1>
              <p className="mt-1 text-[15px] text-ink-2">{t.sub}</p>
            </div>
          </div>

          <form onSubmit={submit} className="grid gap-4" noValidate>
            {mode === 'register' && (
              <Field label="邀请码" htmlFor="invite" error={errFor('invite')} hint="邀请码包含一段托管服务时长。">
                <Input
                  id="invite"
                  value={invite}
                  onChange={(e) => setInvite(normInvite(e.target.value))}
                  placeholder="FISH-XXXX-XXXX"
                  autoComplete="off"
                  spellCheck={false}
                  aria-invalid={!!errFor('invite')}
                  className="font-mono tracking-wider uppercase"
                  required
                />
              </Field>
            )}
            <Field label="邮箱" htmlFor="email" error={errFor('email')}>
              <Input
                id="email"
                type="email"
                inputMode="email"
                autoComplete="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="you@example.com"
                aria-invalid={!!errFor('email')}
                required
              />
            </Field>

            {mode !== 'reset' && (
              <Field
                label={mode === 'register' ? '设置密码' : '密码'}
                htmlFor="password"
                error={errFor('password')}
                hint={mode === 'register' ? '至少 8 位。' : undefined}
                aside={
                  mode === 'login' ? (
                    <Link to={`/reset${email ? `?email=${encodeURIComponent(email)}` : ''}`} className="text-[13px] text-sky">
                      忘记密码
                    </Link>
                  ) : undefined
                }
              >
                <Input
                  id="password"
                  type="password"
                  autoComplete={mode === 'register' ? 'new-password' : 'current-password'}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  minLength={mode === 'register' ? 8 : undefined}
                  required
                />
              </Field>
            )}

            {needCaptcha && (
              <div>
                <Captcha value={captcha} onChange={setCaptcha} onId={setCaptchaId} bump={captchaBump} />
                {errFor('captcha') && (
                  <p role="alert" className="mt-1.5 text-[13px] text-bad">
                    {errFor('captcha')}
                  </p>
                )}
              </div>
            )}

            {mode !== 'login' && (
              <Field
                label="邮箱验证码"
                htmlFor="code"
                error={errFor('code')}
                hint={sent ? `验证码已发到 ${email}，几分钟内有效。` : undefined}
              >
                <div className="flex gap-2">
                  <Input
                    id="code"
                    value={code}
                    onChange={(e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
                    inputMode="numeric"
                    autoComplete="one-time-code"
                    placeholder="6 位数字"
                    aria-invalid={!!errFor('code')}
                    className="flex-1 tabular-nums"
                    required
                  />
                  <Button onClick={sendCode} loading={sending} disabled={cooldown > 0} className="w-[132px]">
                    {cooldown > 0 ? <span className="tabular-nums">{cooldown} 秒后重发</span> : sent ? '重新发送' : '发送验证码'}
                  </Button>
                </div>
              </Field>
            )}

            {mode === 'reset' && (
              <Field label="新密码" htmlFor="password" hint="至少 8 位。" error={errFor('password')}>
                <Input
                  id="password"
                  type="password"
                  autoComplete="new-password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  minLength={8}
                  required
                />
              </Field>
            )}

            {mode === 'register' && (
              <label className="flex items-start gap-2.5 text-sm leading-relaxed text-ink-2">
                <input
                  type="checkbox"
                  checked={agree}
                  onChange={(e) => setAgree(e.target.checked)}
                  className="mt-1 size-4 shrink-0 accent-cobalt"
                />
                <span>
                  我已阅读
                  <Link to="/about#rules" className="mx-0.5 text-sky underline underline-offset-2">
                    托管须知
                  </Link>
                  ， 自己的模型 Key 由我自己负责。
                </span>
              </label>
            )}
            {errFor('agree') && (
              <p role="alert" className="-mt-2 text-[13px] text-bad">
                {errFor('agree')}
              </p>
            )}

            {error && !error.field && (
              <p role="alert" className="rounded-xl bg-bad-soft px-3.5 py-2.5 text-sm text-bad">
                {error.msg}
              </p>
            )}

            <Button type="submit" variant="primary" size="lg" loading={busy} className="mt-1 w-full font-display font-black">
              {mode === 'login' ? '登录' : mode === 'register' ? '注册' : '设置新密码'}
            </Button>
          </form>

          <p className="mt-6 text-center text-sm text-ink-2">
            {mode === 'login' ? (
              <>
                还没有账号？
                <Link to="/register" className="font-semibold text-sky">
                  用邀请码注册
                </Link>
              </>
            ) : (
              <>
                已有账号？
                <Link to="/login" className="font-semibold text-sky">
                  登录
                </Link>
              </>
            )}
          </p>
        </div>
      </main>
    </div>
  )
}
