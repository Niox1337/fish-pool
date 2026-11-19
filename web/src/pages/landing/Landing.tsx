import { useEffect, useRef, useState, type CSSProperties, type FormEvent } from 'react'
import { Link, useNavigate } from 'react-router'
import { ArrowRight, CodeXml, Mail } from 'lucide-react'
import { useMe } from '@/api/hooks'
import { KeySolid, QqPenguin, StarBadge } from '@/components/icons'
import bookSpread from '@/assets/plates/book-spread.webp'
import ctaSparkle from '@/assets/plates/cta-sparkle.webp'
import headlineCloud from '@/assets/plates/headline-cloud.webp'
import illustration from '@/assets/plates/illustration.webp'
import logoMark from '@/assets/plates/logo-mark.webp'
import nextSpread from '@/assets/plates/next-spread.webp'
import nextSpreadEnd from '@/assets/plates/next-spread-end.webp'
import stepsCloud from '@/assets/plates/steps-cloud.webp'
import LandingSections from './LandingSections'
import './landing.css'
import './landing-motion.css'
import './landing-responsive.css'

const STAR_PATH =
  'M24 3.2c1.6 0 2.9.9 3.6 2.4l4.3 9 9.7 1.4c3.3.5 4.6 4.5 2.2 6.8l-7 6.9 1.7 9.7c.6 3.3-2.9 5.8-5.8 4.2L24 39l-8.7 4.6c-2.9 1.6-6.4-.9-5.8-4.2l1.7-9.7-7-6.9c-2.4-2.3-1.1-6.3 2.2-6.8l9.7-1.4 4.3-9c.7-1.5 2-2.4 3.6-2.4Z'

export default function Landing() {
  const navigate = useNavigate()
  const { data: me } = useMe()
  const [email, setEmail] = useState('')
  const root = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const el = root.current
    if (!el || !('IntersectionObserver' in window)) return
    el.setAttribute('data-motion', '')
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (!e.isIntersecting) continue
          e.target.setAttribute('data-in', '')
          io.unobserve(e.target)
        }
      },
      { rootMargin: '0px 0px -12% 0px' },
    )
    el.querySelectorAll('[data-reveal]').forEach((n) => io.observe(n))
    return () => io.disconnect()
  }, [])

  function enroll(e: FormEvent) {
    e.preventDefault()
    const q = email.trim() ? `?email=${encodeURIComponent(email.trim())}` : ''
    navigate(`/register${q}`)
  }

  return (
    <div className="lp" ref={root}>
      <div className="lp-frame">
        <img className="lp-plate lp-book" src={bookSpread} data-plate="assets/plates/book-spread.png" alt="" />
        <img className="lp-plate lp-next" src={nextSpread} data-plate="assets/plates/next-spread.png" alt="" />
        <div className="lp-nav" />
        <div className="lp-paper" aria-hidden />
        <img className="lp-plate lp-logo" src={logoMark} data-plate="assets/plates/logo-mark.png" alt="" />
        <Link to="/" className="lp-brand">
          大肥鱼托管班
        </Link>
        <nav aria-label="主导航" className="contents">
          <ul className="lp-navlinks">
            <li>
              <a href="#how">怎么用</a>
            </li>
            <li>
              <a href="#plugins">插件</a>
            </li>
            <li>
              <a href="#faq">常见问题</a>
            </li>
          </ul>
        </nav>
        <Link to={me ? '/app' : '/login'} className="lp-login">
          {me ? '我的小鱼' : '登录'}
        </Link>

        <img className="lp-plate lp-cloud" src={headlineCloud} data-plate="assets/plates/headline-cloud.png" alt="" />
        <svg className="lp-cloud-star" viewBox="-4 -4 56 54" aria-hidden>
          <path className="lp-cloud-star-halo" d={STAR_PATH} />
          <path className="lp-cloud-star-body" d={STAR_PATH} />
        </svg>
        <h1 className="lp-headline">
          <span>把你的 AI 小鱼</span>
          <span>送来托管</span>
        </h1>
        <p className="lp-subcopy">
          QQ 扫码就能接入，微信也行。自带模型 Key，
          <br />
          装上 AstrBot 插件，服务器交给我们照看。
        </p>
        <img className="lp-plate lp-sparkle" src={ctaSparkle} data-plate="assets/plates/cta-sparkle.png" alt="" />
        <form className="lp-form" onSubmit={enroll}>
          <label className="lp-field">
            <Mail aria-hidden strokeWidth={1.8} />
            <span className="sr-only">邮箱</span>
            <input
              type="email"
              inputMode="email"
              autoComplete="email"
              placeholder="请输入你的邮箱"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
          </label>
          <button type="submit" className="lp-cta">
            带小鱼入学
          </button>
        </form>
        <p className="lp-loginlink">
          已有账号？<Link to="/login">登录</Link>
        </p>
        <img
          className="lp-plate lp-art"
          src={illustration} data-plate="assets/plates/illustration.png"
          alt="梗Wiki 的蓝发小老师在阅读角给四条大肥鱼小朋友讲绘本"
          fetchPriority="high"
        />

        <div className="lp-paper lp-paper-steps" aria-hidden />
        <img className="lp-plate lp-steps-cloud" src={stepsCloud} data-plate="assets/plates/steps-cloud.png" alt="" />
        <h2 id="how" className="lp-steps-title">
          三步入学
        </h2>
        <span className="lp-divider" aria-hidden />
        <ol className="contents">
          <li className="lp-step lp-step-1" data-reveal style={{ "--i": 0 } as CSSProperties}>
            <StarBadge n={1} className="lp-step-badge" />
            <QqPenguin className="lp-step-icon" />
            <span className="lp-step-text">
              扫码接 QQ
              <small>（AppID 自动填好）</small>
            </span>
          </li>
          <li className="lp-arrow lp-arrow-1" data-reveal style={{ "--i": 1 } as CSSProperties} aria-hidden>
            <ArrowRight strokeWidth={3} />
          </li>
          <li className="lp-step lp-step-2" data-reveal style={{ "--i": 2 } as CSSProperties}>
            <StarBadge n={2} className="lp-step-badge" />
            <KeySolid className="lp-step-icon" />
            <span className="lp-step-text">
              填上你的
              <br />
              模型 Key
            </span>
          </li>
          <li className="lp-arrow lp-arrow-2" data-reveal style={{ "--i": 3 } as CSSProperties} aria-hidden>
            <ArrowRight strokeWidth={3} />
          </li>
          <li className="lp-step lp-step-3" data-reveal style={{ "--i": 4 } as CSSProperties}>
            <StarBadge n={3} className="lp-step-badge" />
            <CodeXml className="lp-step-icon" strokeWidth={2.4} />
            <span className="lp-step-text">
              贴插件
              <br />
              仓库地址
            </span>
          </li>
        </ol>
      </div>
      <div className="lp-next-end" aria-hidden>
        <img src={nextSpreadEnd} data-plate="assets/plates/next-spread-end.png" alt="" />
      </div>
      <LandingSections />
    </div>
  )
}
