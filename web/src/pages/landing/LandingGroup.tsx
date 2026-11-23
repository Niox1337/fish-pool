import { Copy, ExternalLink } from 'lucide-react'
import qqGroup from '@/assets/brand/qq-group.webp'
import { Pupil } from '@/components/Pupil'
import { copyText } from '@/pages/console/admin/copy'
import './landing-group.css'

const QQ_GROUP_NUMBER = '1044394495'
const QQ_GROUP_LINK = 'https://qm.qq.com/q/t0MoBb1C3C'

/** The QQ group section: QR code, group number with copy, and a link that opens QQ. */
export default function LandingGroup() {
  return (
    <section id="group" data-reveal data-loop className="lg" aria-labelledby="lg-title">
      <Pupil mood="wave" size={168} className="lg-whale" />
      <div className="lg-card">
        <span className="lg-tape lg-tape-a" aria-hidden />
        <span className="lg-tape lg-tape-b" aria-hidden />
        <div className="lg-text">
          <h2 id="lg-title" className="lg-title">
            来班级群里聊聊
          </h2>
          <p className="lg-body">
            加入<strong>大肥鱼开发交流群</strong>，用手机 QQ 扫一扫二维码，或者搜索群号。
          </p>
          <div className="lg-number">
            <span className="lg-number-label">群号</span>
            <span className="lg-number-value">{QQ_GROUP_NUMBER}</span>
            <button type="button" className="lg-copy" onClick={() => copyText(QQ_GROUP_NUMBER, '群号')}>
              <Copy aria-hidden strokeWidth={2.4} />
              复制
            </button>
          </div>
          <a className="lg-open" href={QQ_GROUP_LINK} target="_blank" rel="noopener noreferrer">
            打开 QQ 加入
            <ExternalLink aria-hidden strokeWidth={2.6} />
          </a>
        </div>
        <figure className="lg-photo">
          <img src={qqGroup} alt="大肥鱼开发交流群的 QQ 群二维码" width={480} height={480} loading="lazy" />
          <figcaption>用手机 QQ 扫码加入</figcaption>
        </figure>
      </div>
    </section>
  )
}
