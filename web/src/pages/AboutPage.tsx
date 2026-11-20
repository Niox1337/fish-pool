import { useEffect, type ReactNode } from 'react'
import { Link, useLocation } from 'react-router'
import { useMe } from '@/api/hooks'
import { Pupil } from '@/components/Pupil'
import { StarBadge } from '@/components/icons'
import logoMark from '@/assets/plates/logo-mark.webp'

const rules: { title: string; body: ReactNode }[] = [
  {
    title: '凭邀请码开通服务',
    body: (
      <>
        现在只能用邀请码注册。每个邀请码带着一段服务天数，注册或在账号页兑换后生效。还没到期时从到期日往后加，已经到期时从兑换当天开始算。服务到期后小鱼会停下，不能再收新小鱼、启动小鱼或装插件，设置、插件和聊天记录都会保留，兑换新的邀请码就能接着用。
      </>
    ),
  },
  {
    title: '自带模型 Key',
    body: (
      <>
        小鱼说话用的是你自己的大模型 Key，任何兼容 OpenAI 接口的服务商都行，比如 DeepSeek、OpenAI、SiliconFlow、Moonshot、智谱。托管班不提供模型额度，模型费用由你和服务商之间结算。
      </>
    ),
  },
  {
    title: '接入 QQ',
    body: (
      <>
        用手机 QQ 扫码，就能创建一个 QQ 官方机器人并接到小鱼上，AppID 和密钥会自动保存。已经有 QQ 官方机器人的话，也可以手动填写 AppID 和 AppSecret。
      </>
    ),
  },
  {
    title: '接入微信',
    body: (
      <>
        用微信扫码绑定。微信这边只支持私聊，不能拉进群聊。对方给小鱼发消息后的 24 小时内小鱼才能回复，超过 24 小时需要对方再发一条消息。
      </>
    ),
  },
  {
    title: '插件兼容',
    body: (
      <>
        可以从 GitHub 安装 AstrBot 插件，支持的是插件接口里有文档说明的一部分。插件用到暂不支持的功能时会被停下，状态里会写明是哪个功能，其他插件和小鱼本身不受影响。
      </>
    ),
  },
  {
    title: '资源额度',
    body: (
      <>
        每个账号有内存、存储、小鱼条数和插件个数四项额度。用量按来源分别统计：小鱼本体、聊天记录和每个插件各算一份，在资源用量页能看到明细。内存持续超过额度时，先停下占内存最多的插件。存储超过额度时，新的插件装不上。
      </>
    ),
  },
  {
    title: '密钥加密保存',
    body: (
      <>
        模型 Key 和 QQ、微信的平台密钥都用 AES-256-GCM 加密后保存，页面上只显示最后几位，管理员也看不到完整内容。
      </>
    ),
  },
]

export default function AboutPage() {
  const loc = useLocation()
  const { data: me } = useMe()

  useEffect(() => {
    if (!loc.hash) {
      window.scrollTo(0, 0)
      return
    }
    const el = document.getElementById(decodeURIComponent(loc.hash.slice(1)))
    if (el) requestAnimationFrame(() => el.scrollIntoView({ block: 'start' }))
  }, [loc.hash])

  return (
    <div className="min-h-dvh bg-paper">
      <header className="bg-cobalt text-white">
        <div className="mx-auto flex h-16 max-w-4xl items-center gap-3 px-4 sm:px-8">
          <Link to="/" className="flex items-center gap-2.5">
            <img src={logoMark} alt="" className="h-8 w-auto" />
            <span className="font-display text-lg font-black">大肥鱼托管班</span>
          </Link>
          <nav aria-label="页面" className="ml-auto flex items-center gap-1 text-sm font-semibold">
            <Link to="/" className="hidden rounded-lg px-3 py-1.5 text-white/80 transition-colors duration-150 hover:bg-white/10 hover:text-white sm:inline-flex">
              首页
            </Link>
            <Link
              to={me ? '/app' : '/login'}
              className="inline-flex h-9 items-center rounded-xl border border-star-2 bg-star px-4 text-ink shadow-[var(--shadow-star)] transition-[filter] duration-150 hover:brightness-[1.04]"
            >
              {me ? '进入控制台' : '登录'}
            </Link>
          </nav>
        </div>
        <div className="mx-auto flex max-w-4xl items-end gap-5 px-4 pt-6 pb-10 sm:px-8 sm:pt-8 sm:pb-12">
          <div className="min-w-0 flex-1">
            <h1 className="font-display text-3xl font-black sm:text-4xl">关于托管班</h1>
            <p className="mt-3 max-w-[56ch] text-[15px] leading-relaxed text-white/80 sm:text-base">
              大肥鱼托管班帮你托管 QQ 和微信聊天机器人。每个机器人是一条小鱼，托管班给它分好座位、看着它运行，你只需要扫码接入、选好人设和模型、装上想要的插件。
            </p>
          </div>
          <Pupil mood="portrait" size={112} className="hidden shrink-0 sm:inline-block" />
        </div>
      </header>

      <main className="mx-auto max-w-4xl px-4 py-10 sm:px-8 sm:py-14">
        <section aria-labelledby="rules" className="scroll-mt-6">
          <h2 id="rules" className="scroll-mt-6 font-display text-2xl font-black text-ink">
            托管须知
          </h2>
          <p className="mt-2 max-w-[60ch] text-[15px] leading-relaxed text-ink-2">注册前请读一遍，这些是托管班现在的规矩。</p>
          <ol className="mt-6 grid gap-5">
            {rules.map((r, i) => (
              <li key={r.title} className="grid grid-cols-[2.25rem_minmax(0,1fr)] gap-x-4">
                <StarBadge n={i + 1} className="size-9 text-[15px]" />
                <div>
                  <h3 className="pt-1.5 font-semibold text-ink">{r.title}</h3>
                  <p className="mt-1 max-w-[64ch] text-[15px] leading-relaxed text-ink-2">{r.body}</p>
                </div>
              </li>
            ))}
          </ol>
        </section>

        <section aria-labelledby="credits" className="mt-14 border-t border-paper-3 pt-10">
          <h2 id="credits" className="font-display text-2xl font-black text-ink">
            项目说明
          </h2>
          <p className="mt-4 text-[15px] leading-relaxed text-ink-2">
            托管班用 AstrBot 本体运行插件，插件宿主随 AstrBot 以 AGPL-3.0 开源，源码见{' '}
            <a href="https://github.com/Niox1337/fish-pool/tree/main/sidecar" target="_blank" rel="noreferrer" className="text-sky underline underline-offset-2">
              项目仓库的 sidecar 目录
            </a>
            。托管班和 AstrBot 项目没有隶属或合作关系。
          </p>
        </section>
      </main>

      <footer className="border-t border-paper-3">
        <div className="mx-auto flex max-w-4xl flex-wrap items-center gap-x-5 gap-y-2 px-4 py-6 text-sm text-ink-3 sm:px-8">
          <span className="font-display font-black text-ink-2">大肥鱼托管班</span>
          <Link to="/" className="hover:text-ink">
            回到首页
          </Link>
          <Link to={me ? '/app' : '/login'} className="hover:text-ink">
            {me ? '进入控制台' : '登录'}
          </Link>
        </div>
      </footer>
    </div>
  )
}
