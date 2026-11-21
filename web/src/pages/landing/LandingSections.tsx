import { Link } from 'react-router'
import { ArrowRight, Check, Plus } from 'lucide-react'
import whaleHappy from '@/assets/whale/happy.webp'
import whalePortrait from '@/assets/whale/portrait.webp'
import whaleSurprised from '@/assets/whale/surprised.webp'
import { Pupil } from '@/components/Pupil'
import { FAQ } from './faq'
import LandingGroup from './LandingGroup'
import PluginDemo from './PluginDemo'
import './landing-sections.css'

const PLUGIN_MARKET = 'https://cloud.astrbot.app/market'

export default function LandingSections() {
  return (
    <div className="ls">
      <section data-reveal className="ls-book" aria-labelledby="ls-who">
        <div className="ls-page">
          <h2 id="ls-who" className="ls-title">
            第一次养小鱼
          </h2>
          <ul className="ls-checks">
            <li>
              <Check aria-hidden strokeWidth={3} />
              QQ 用手机扫一下码，AppID 和密钥自动填好
            </li>
            <li>
              <Check aria-hidden strokeWidth={3} />
              人设像写自我介绍一样填，参数都有滑块
            </li>
            <li>
              <Check aria-hidden strokeWidth={3} />
              不用租服务器，也不用敲命令行
            </li>
          </ul>
          <img className="ls-whale" src={whaleHappy} alt="" width={256} height={256} loading="lazy" />
        </div>
        <div className="ls-page">
          <h2 className="ls-title">带着 AstrBot 插件来</h2>
          <ul className="ls-checks">
            <li>
              <Check aria-hidden strokeWidth={3} />
              插件市场就在控制台里，点一下就装
            </li>
            <li>
              <Check aria-hidden strokeWidth={3} />
              插件配置照着插件自己的格式生成表单
            </li>
            <li>
              <Check aria-hidden strokeWidth={3} />
              日志按来源分开，哪个插件出错一眼看到
            </li>
          </ul>
          <img className="ls-whale" src={whalePortrait} alt="" width={256} height={256} loading="lazy" />
        </div>
      </section>

      <section data-reveal id="plugins" className="ls-book" aria-labelledby="ls-plugins">
        <div className="ls-page">
          <h2 id="ls-plugins" className="ls-title">
            点一下，装上新本领
          </h2>
          <p className="ls-body">
            小鱼的插件页里就是 AstrBot 插件市场，搜到喜欢的插件点一下安装。托管班会装好依赖、启动插件，并把安装过程写进日志。
          </p>
          <p className="ls-body ls-muted">每个插件单独运行，一个插件出问题不会拖累小鱼本体。</p>
        </div>
        <PluginDemo />
      </section>

      <section className="ls-note" aria-labelledby="ls-invite">
        <img className="ls-note-whale" src={whaleSurprised} alt="" width={256} height={256} loading="lazy" />
        <div>
          <h2 id="ls-invite" className="ls-note-title">
            开通托管需要邀请码
          </h2>
          <p className="ls-body">每个邀请码都带有服务天数。到期时小鱼会停止运行，设置和插件会保留。兑换新的邀请码后即可继续托管。</p>
        </div>
      </section>

      <section id="faq" className="ls-faq" aria-labelledby="ls-faq-title">
        <h2 id="ls-faq-title" className="ls-section-title">
          常见问题
        </h2>
        <div className="ls-faq-list">
          {FAQ.map((item) => (
            <details key={item.q} className="ls-faq-item">
              <summary>
                {item.q}
                <Plus aria-hidden strokeWidth={3} />
              </summary>
              <p>{item.a}</p>
            </details>
          ))}
        </div>
      </section>

      <LandingGroup />

      <section data-loop className="ls-end" aria-labelledby="ls-end-title">
        <Pupil mood="wave" size={144} />
        <h2 id="ls-end-title" className="ls-end-title">
          准备好了就带小鱼来
        </h2>
        <Link to="/register" className="ls-end-cta">
          开始托管
          <ArrowRight aria-hidden strokeWidth={3} />
        </Link>
      </section>

      <footer className="ls-foot">
        <div className="ls-foot-row">
          <span className="ls-foot-brand">大肥鱼托管班</span>
          <nav aria-label="页脚" className="ls-foot-links">
            <Link to="/about">关于</Link>
            <Link to="/login">登录</Link>
            <a href="#group">QQ 群</a>
            <a href={PLUGIN_MARKET} target="_blank" rel="noreferrer">
              插件市场
            </a>
          </nav>
        </div>
        <p className="ls-foot-note">插件由 AstrBot 本体运行，托管班与 AstrBot 官方无关。</p>
      </footer>
    </div>
  )
}
