import { Link } from 'react-router'
import { ArrowRight, Check, ExternalLink, Plus } from 'lucide-react'
import whaleHappy from '@/assets/whale/happy.webp'
import whalePortrait from '@/assets/whale/portrait.webp'
import whaleSurprised from '@/assets/whale/surprised.webp'
import { Pupil } from '@/components/Pupil'
import './landing-sections.css'

const PLUGIN_MARKET = 'https://cloud.astrbot.app/market'

const FAQ: { q: string; a: string }[] = [
  {
    q: '需要自己准备模型吗？',
    a: '需要。填上任意 OpenAI 兼容接口的地址和 Key 就行，小鱼用你自己的模型说话，托管班不提供模型额度。',
  },
  {
    q: '微信能接吗？',
    a: '能。用微信扫码绑定就好。目前只支持私聊，并且只能在对方发来消息后的 24 小时内回复。',
  },
  {
    q: '我的 Key 放在这里安全吗？',
    a: '模型 Key、QQ 的 AppSecret 和微信凭证都经过 AES-256-GCM 加密后才写进数据库，页面上也只显示末尾几位。',
  },
  {
    q: 'AstrBot 插件都能装吗？',
    a: '常用的 AstrBot 插件接口已经实现，很多插件贴上仓库地址就能运行。插件用到暂时不支持的功能时会停下来，并写明是哪一项功能。',
  },
  {
    q: '资源怎么算？',
    a: '每位同学有自己的内存和存储配额。用量按来源分开显示：小鱼本体、聊天记录，还有每一个插件各占多少。',
  },
  {
    q: '怎么拿到邀请码？',
    a: '托管服务目前需要管理员发放的邀请码。注册后开始计算服务期限。',
  },
]

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
              插件市场里的仓库地址，贴进来就装
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
          <h2 id="ls-plugins" className="ls-title">一行地址，装上新本领</h2>
          <p className="ls-body">
            在 <a href={PLUGIN_MARKET} target="_blank" rel="noreferrer">AstrBot 插件市场<ExternalLink aria-hidden /></a>{' '}
            找到喜欢的插件，复制它的 GitHub 仓库地址，贴到小鱼的插件页。托管班会装好依赖、启动插件，并把安装过程写进日志。
          </p>
          <p className="ls-body ls-muted">每个插件单独运行，一个插件出问题不会拖累小鱼本体。</p>
        </div>
        <div className="ls-page ls-page-demo">
          <span className="ls-demo-tag">示例</span>
          <div className="ls-demo" aria-hidden>
            <div className="ls-demo-field">
              <span>github.com/</span>
              <span className="ls-demo-typed">someone/astrbot_plugin_weather</span>
            </div>
            <span className="ls-demo-add">
              <Plus strokeWidth={3} />
              装上
            </span>
          </div>
          <ol className="ls-shelf" aria-hidden>
            <li>
              <span className="ls-spine ls-spine-blue" />
              <span className="ls-shelf-name">天气预报</span>
              <span className="ls-chip ls-chip-ok">运行中</span>
            </li>
            <li>
              <span className="ls-spine ls-spine-pink" />
              <span className="ls-shelf-name">每日一言</span>
              <span className="ls-chip ls-chip-ok">运行中</span>
            </li>
            <li>
              <span className="ls-spine ls-spine-star" />
              <span className="ls-shelf-name">群聊小游戏</span>
              <span className="ls-chip ls-chip-warn">安装中</span>
            </li>
          </ol>
          <div className="ls-usage" aria-hidden>
            <p className="ls-usage-label">内存用量，按来源</p>
            <div className="ls-usage-bar">
              <span style={{ flexGrow: 5 }} className="ls-seg-core" />
              <span style={{ flexGrow: 2 }} className="ls-seg-chat" />
              <span style={{ flexGrow: 3 }} className="ls-seg-a" />
              <span style={{ flexGrow: 1.5 }} className="ls-seg-b" />
              <span style={{ flexGrow: 6 }} className="ls-seg-free" />
            </div>
            <ul className="ls-legend">
              <li><i className="ls-seg-core" />小鱼本体</li>
              <li><i className="ls-seg-chat" />聊天记录</li>
              <li><i className="ls-seg-a" />天气预报</li>
              <li><i className="ls-seg-b" />每日一言</li>
            </ul>
          </div>
        </div>
      </section>

      <section className="ls-note" aria-labelledby="ls-invite">
        <img className="ls-note-whale" src={whaleSurprised} alt="" width={256} height={256} loading="lazy" />
        <div>
          <h2 id="ls-invite" className="ls-note-title">
            开通托管需要邀请码
          </h2>
          <p className="ls-body">
            每个邀请码都带有服务天数。到期时小鱼会停止运行，设置和插件会保留。兑换新的邀请码后即可继续托管。
          </p>
        </div>
      </section>

      <section id="faq" className="ls-faq" aria-labelledby="ls-faq-title">
        <h2 id="ls-faq-title" className="ls-section-title">常见问题</h2>
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

      <section className="ls-end" aria-labelledby="ls-end-title">
        <Pupil mood="wave" size={144} />
        <h2 id="ls-end-title" className="ls-end-title">准备好了就带小鱼来</h2>
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
