export type ModelProvider = {
  id: string
  name: string
  /** Where users get a key. Providers without one hand out keys elsewhere. */
  platform_url?: string
  base_url: string
  models: string[]
}

export const modelProviders: ModelProvider[] = [
  // DeepSeek models with prepaid credit handed out as event rewards
  {
    id: 'gengwiki',
    name: '活动获取',
    base_url: 'https://gateway.gengwiki.com/v1',
    models: ['deepseek-flash', 'deepseek-v4-pro'],
  },
  // https://api-docs.deepseek.com/api/create-chat-completion/
  {
    id: 'deepseek',
    name: '深度求索（DeepSeek）',
    platform_url: 'https://platform.deepseek.com/',
    base_url: 'https://api.deepseek.com',
    models: ['deepseek-flash', 'deepseek-v4-pro'],
  },
  // https://developers.openai.com/api/docs/guides/deployment-checklist
  {
    id: 'openai',
    name: 'OpenAI',
    platform_url: 'https://platform.openai.com/api-keys',
    base_url: 'https://api.openai.com/v1',
    models: ['gpt-6-luna', 'gpt-6.1-terra', 'gpt-6-sol'],
  },
  // https://cloud.siliconflow.cn/open/models?types=chat
  {
    id: 'siliconflow',
    name: '硅基流动（SiliconFlow）',
    platform_url: 'https://cloud.siliconflow.cn/',
    base_url: 'https://api.siliconflow.cn/v1',
    models: ['deepseek-ai/DeepSeek-V4.1-Flash', 'deepseek-ai/DeepSeek-V4-Flash', 'deepseek-ai/DeepSeek-V4-Pro'],
  },
  // https://platform.kimi.com/docs/models
  {
    id: 'moonshot',
    name: '月之暗面（Moonshot AI）',
    platform_url: 'https://platform.kimi.com/',
    base_url: 'https://api.moonshot.cn/v1',
    models: ['kimi-k3', 'kimi-k3.1-pro', 'kimi-k3.1-pro-highspeed'],
  },
  // https://docs.bigmodel.cn/cn/guide/start/model-overview
  {
    id: 'zhipu',
    name: '智谱（Z.AI）',
    platform_url: 'https://open.bigmodel.cn/',
    base_url: 'https://open.bigmodel.cn/api/paas/v4',
    models: ['glm-5.3-flash', 'glm-5.3-flashx', 'glm-5.3'],
  },
  // https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope
  {
    id: 'qwen',
    name: '千问（Qwen）',
    platform_url: 'https://bailian.console.aliyun.com/',
    base_url: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    models: ['qwen3.8-flash', 'qwen3.7-plus', 'qwen3.8-max'],
  },
  // https://console.volcengine.com/ark/region:cn-beijing/docs/model-list
  {
    id: 'volcengine',
    name: '火山引擎（Volcano Engine）',
    platform_url: 'https://console.volcengine.com/ark/region:ark+cn-beijing/apikey',
    base_url: 'https://ark.cn-beijing.volces.com/api/v3',
    models: ['doubao-seed-2-1-lite-260915', 'doubao-seed-2-0-mini-260428', 'doubao-seed-2-1-pro-260915'],
  },
  // https://mimo.mi.com/docs/en-US/api/chat/openai-api-post
  {
    id: 'mimo',
    name: '小米 MiMo（Xiaomi MiMo）',
    platform_url: 'https://platform.xiaomimimo.com/',
    base_url: 'https://api.xiaomimimo.com/v1',
    models: ['mimo-v2.6-flash', 'mimo-v2.6-pro', 'mimo-v2.6-pro-ultraspeed'],
  },
  // https://aistudio.google.com/docs/openai
  {
    id: 'google',
    name: '谷歌（Google Gemini）',
    platform_url: 'https://aistudio.google.com/apikey',
    base_url: 'https://generativelanguage.googleapis.com/v1beta/openai',
    models: ['gemini-4-hydrogen', 'gemini-4-neon', 'gemini-4-carbon'],
  },
  // https://platform.claude.com/docs/en/models/overview
  {
    id: 'anthropic',
    name: 'Anthropic（Claude）',
    platform_url: 'https://platform.claude.com/',
    base_url: 'https://api.anthropic.com/v1',
    models: ['claude-haiku-5-5', 'claude-sonnet-5-5', 'claude-opus-5-5'],
  },
  // https://docs.x.ai/developers/models
  {
    id: 'grok',
    name: 'Grok（xAI）',
    platform_url: 'https://console.x.ai/',
    base_url: 'https://api.x.ai/v1',
    models: ['grok-4.3', 'grok-4.7', 'grok-4.6'],
  },
  // https://platform.minimax.cn/docs/api-reference/text-chat-openai
  {
    id: 'minimax',
    name: '稀宇科技（MiniMax）',
    platform_url: 'https://platform.minimax.cn/',
    base_url: 'https://api.minimax.cn/v1',
    models: ['MiniMax-M3.1-Flash', 'MiniMax-M3'],
  },
  // https://longcat.chat/platform/docs/zh/
  {
    id: 'longcat',
    name: '美团 LongCat（Meituan LongCat）',
    platform_url: 'https://longcat.chat/platform/',
    base_url: 'https://api.longcat.chat/openai/v1',
    models: ['LongCat-2.5'],
  },
]

export function providerForEndpoint(baseUrl: string) {
  const url = baseUrl.replace(/\/+$/, '')
  return modelProviders.find((provider) =>
    provider.base_url === url ||
    (provider.id === 'deepseek' && url === `${provider.base_url}/v1`) ||
    (provider.id === 'minimax' && ['https://api.minimaxi.com/v1', 'https://api.minimax.io/v1'].includes(url)) ||
    (provider.id === 'qwen' && /^https:\/\/[a-z0-9-]+\.cn-beijing\.maas\.aliyuncs\.com\/compatible-mode\/v1$/.test(url)),
  )
}
