import { FAQ } from './pages/landing/faq'

export const SITE_URL = 'https://fish.niox.lol'
export const SITE_NAME = '大肥鱼托管班'
const SUFFIX = `｜${SITE_NAME}`

export interface PageSeo {
  title: string
  description: string
  /** Structured data objects written as JSON-LD. */
  jsonLd?: object[]
}

const KEYWORDS = [
  '大肥鱼托管班',
  'QQ机器人',
  'QQ机器人托管',
  'QQ官方机器人',
  '微信机器人',
  'AI聊天机器人',
  '聊天机器人托管',
  'AstrBot',
  'AstrBot插件',
  '机器人一键部署',
].join(',')

export const OG_IMAGE = `${SITE_URL}/og-image.jpg`

const organization = {
  '@type': 'Organization',
  name: SITE_NAME,
  url: `${SITE_URL}/`,
  logo: `${SITE_URL}/apple-touch-icon.png`,
}

/** Pages that are rendered to static HTML at build time, keyed by path. */
export const PRERENDERED: Record<string, PageSeo> = {
  '/': {
    title: `${SITE_NAME}｜QQ机器人、微信AI聊天机器人一键托管`,
    description:
      '大肥鱼托管班是 QQ 和微信 AI 聊天机器人的托管平台。手机扫码接入 QQ 官方机器人和微信，填上自己的大模型 Key，在插件市场一键安装 AstrBot 插件，不用租服务器，也不用敲命令行。',
    jsonLd: [
      { '@context': 'https://schema.org', '@type': 'WebSite', name: SITE_NAME, url: `${SITE_URL}/`, inLanguage: 'zh-CN' },
      { '@context': 'https://schema.org', ...organization },
      {
        '@context': 'https://schema.org',
        '@type': 'WebApplication',
        name: SITE_NAME,
        url: `${SITE_URL}/`,
        applicationCategory: 'CommunicationApplication',
        operatingSystem: 'Web',
        inLanguage: 'zh-CN',
        image: OG_IMAGE,
        description: 'QQ 和微信 AI 聊天机器人托管，扫码接入，自带大模型 Key，支持 AstrBot 插件。',
        provider: organization,
      },
      {
        '@context': 'https://schema.org',
        '@type': 'FAQPage',
        mainEntity: FAQ.map((f) => ({
          '@type': 'Question',
          name: f.q,
          acceptedAnswer: { '@type': 'Answer', text: f.a },
        })),
      },
    ],
  },
  '/about': {
    title: `托管须知与插件兼容说明${SUFFIX}`,
    description:
      '大肥鱼托管班的托管须知：邀请码开通、自带模型 Key、QQ 和微信接入方式、AstrBot 插件兼容范围、内存和存储额度，以及密钥如何加密保存。',
    jsonLd: [
      {
        '@context': 'https://schema.org',
        '@type': 'BreadcrumbList',
        itemListElement: [
          { '@type': 'ListItem', position: 1, name: SITE_NAME, item: `${SITE_URL}/` },
          { '@type': 'ListItem', position: 2, name: '关于托管班', item: `${SITE_URL}/about` },
        ],
      },
    ],
  },
}

const APP_TITLES: [string, string][] = [
  ['/login', '登录'],
  ['/register', '注册'],
  ['/reset', '重置密码'],
  ['/app', '我的小鱼'],
]

/** Document title for a client side route. */
export function titleFor(path: string): string {
  const page = PRERENDERED[path]
  if (page) return page.title
  const match = APP_TITLES.find(([prefix]) => path === prefix || path.startsWith(`${prefix}/`))
  return match ? `${match[1]}${SUFFIX}` : SITE_NAME
}

function escapeAttr(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

/** Head tags of a prerendered page. */
export function headTags(path: string): string {
  const page = PRERENDERED[path]
  const url = `${SITE_URL}${path}`
  const tags = [
    `<title>${escapeAttr(page.title)}</title>`,
    `<meta name="description" content="${escapeAttr(page.description)}" />`,
    `<meta name="keywords" content="${escapeAttr(KEYWORDS)}" />`,
    `<link rel="canonical" href="${url}" />`,
    `<meta property="og:type" content="website" />`,
    `<meta property="og:site_name" content="${SITE_NAME}" />`,
    `<meta property="og:locale" content="zh_CN" />`,
    `<meta property="og:url" content="${url}" />`,
    `<meta property="og:title" content="${escapeAttr(page.title)}" />`,
    `<meta property="og:description" content="${escapeAttr(page.description)}" />`,
    `<meta property="og:image" content="${OG_IMAGE}" />`,
    `<meta property="og:image:width" content="1200" />`,
    `<meta property="og:image:height" content="835" />`,
    `<meta name="twitter:card" content="summary_large_image" />`,
    `<meta itemprop="name" content="${escapeAttr(page.title)}" />`,
    `<meta itemprop="description" content="${escapeAttr(page.description)}" />`,
    `<meta itemprop="image" content="${OG_IMAGE}" />`,
  ]
  for (const data of page.jsonLd ?? []) {
    tags.push(`<script type="application/ld+json">${JSON.stringify(data).replace(/</g, '\\u003c')}</script>`)
  }
  return tags.join('\n    ')
}
