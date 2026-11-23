import type { ReactNode } from 'react'
import { Link } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, KeyRound, Undo2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { keys, useBotMutation, useLlmKeys } from '@/api/hooks'
import { providerForEndpoint } from '@/api/modelProviders'
import type { Bot, BotSettings } from '@/api/types'
import { Button, Field, Input, Skeleton, Textarea } from '@/components/ui'
import { useFieldId } from '@/lib/fieldId'
import { ChipsInput, RangeField, Select, SwitchRow } from './shared'
import { useErrorToast } from './errors'
import ModelField from '../ModelField'

const PROMPT_LIMIT = 8000

function Block({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <h2 className="font-display text-lg font-black text-ink">{title}</h2>
      <div className="mt-5 grid gap-5">{children}</div>
    </section>
  )
}

const intOk = (n: number, min: number, max: number) => Number.isInteger(n) && n >= min && n <= max

export default function Persona({
  bot,
  draft,
  setDraft,
}: {
  bot: Bot
  draft: BotSettings | null
  setDraft: (s: BotSettings | null) => void
}) {
  const s = draft ?? bot.settings
  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(bot.settings)
  const llmKeys = useLlmKeys()
  const queryClient = useQueryClient()
  const showError = useErrorToast()
  const save = useBotMutation(bot.id, (settings: BotSettings) => api.updateBot(bot.id, { settings }))
  const ids = {
    name: useFieldId('p-name'),
    prompt: useFieldId('p-prompt'),
    greeting: useFieldId('p-greet'),
    key: useFieldId('p-key'),
    model: useFieldId('p-model'),
    temp: useFieldId('p-temp'),
    tokens: useFieldId('p-tokens'),
    turns: useFieldId('p-turns'),
    prefixes: useFieldId('p-prefix'),
    admins: useFieldId('p-admins'),
    block: useFieldId('p-block'),
    rate: useFieldId('p-rate'),
  }

  function set<K extends keyof BotSettings>(k: K, patch: Partial<BotSettings[K]>) {
    setDraft({ ...s, [k]: { ...s[k], ...patch } })
  }

  const keyList = llmKeys.data ?? []
  const key = keyList.find((k) => k.id === s.llm.key_id)
  const provider = key && providerForEndpoint(key.base_url)
  const presetProvider = provider?.models.includes(s.llm.model || key?.model || '') ? provider.id : undefined
  const fixedTemperature =
    provider?.id === 'anthropic' || presetProvider === 'moonshot' || presetProvider === 'minimax' || presetProvider === 'longcat'
  const maxTemperature =
    presetProvider === 'zhipu' || presetProvider === 'grok' ? 1 : presetProvider === 'mimo' ? 1.5 : presetProvider === 'qwen' ? 1.9 : 2
  const missingKey = !!s.llm.key_id && llmKeys.isSuccess && !key
  const tokensErr = intOk(s.llm.max_tokens, 1, 32768) ? '' : '请输入 1 到 32768 之间的整数'
  const rateErr = intOk(s.behavior.rate_limit_per_minute, 1, 600) ? '' : '请输入 1 到 600 之间的整数'
  const nameErr = !s.persona.name.trim() || [...s.persona.name].length > 32 ? '人设名称须为 1 到 32 个字符' : ''
  const promptLen = [...s.persona.system_prompt].length
  const promptErr = promptLen > PROMPT_LIMIT ? '系统提示词最多 8000 个字符' : ''
  const greetingErr = [...s.persona.greeting].length > 500 ? '开场白最多 500 个字符' : ''
  const invalid = !!(tokensErr || rateErr || nameErr || promptErr || greetingErr)

  function submit() {
    if (invalid) return
    save.mutate(s, {
      onSuccess: (updated) => {
        queryClient.setQueryData(keys.bot(bot.id), updated)
        setDraft(null)
        toast.success('保存好了，小鱼马上换上新设置')
      },
      onError: (e) => showError(e, '没能保存，稍后再试'),
    })
  }

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        submit()
      }}
      noValidate
      className="grid gap-4 lg:grid-cols-[minmax(0,1.12fr)_minmax(0,1fr)]"
    >
      <Block title="人设">
        <Field label="名字" htmlFor={ids.name} error={nameErr || undefined}>
          <Input
            id={ids.name}
            value={s.persona.name}
            aria-invalid={!!nameErr || undefined}
            onChange={(e) => set('persona', { name: e.target.value })}
          />
        </Field>
        <Field
          label="系统提示词"
          htmlFor={ids.prompt}
          hint="如果安装了人设插件，建议清空此处。"
          error={promptErr || undefined}
          aside={
            <span className="text-[13px] text-ink-3 tabular-nums">
              {promptLen} / {PROMPT_LIMIT}
            </span>
          }
        >
          <Textarea
            id={ids.prompt}
            rows={10}
            value={s.persona.system_prompt}
            onChange={(e) => set('persona', { system_prompt: e.target.value })}
            aria-invalid={!!promptErr || undefined}
          />
        </Field>
        <Field label="开场白" htmlFor={ids.greeting} error={greetingErr || undefined}>
          <Input
            id={ids.greeting}
            value={s.persona.greeting}
            placeholder="大家好，我是新来的小鱼"
            onChange={(e) => set('persona', { greeting: e.target.value })}
          />
        </Field>
      </Block>

      <Block title="模型">
        <Field
          label="模型 Key"
          htmlFor={ids.key}
          error={missingKey ? '之前选的 Key 已经删掉了，请重新选一个' : undefined}
          aside={
            keyList.length > 0 && (
              <Link to="/app/keys" className="text-[13px] font-semibold text-sky hover:underline">
                管理 Key
              </Link>
            )
          }
        >
          {llmKeys.isLoading ? (
            <Skeleton className="h-11" />
          ) : keyList.length === 0 ? (
            <div className="flex flex-wrap items-center gap-3 rounded-xl border border-dashed border-paper-3 bg-paper/60 px-4 py-3">
              <KeyRound className="size-5 shrink-0 text-ink-3" aria-hidden />
              <p className="min-w-0 flex-1 text-sm text-ink-2">暂无模型 Key。</p>
              <Link
                to="/app/keys"
                className="inline-flex h-9 items-center rounded-lg border border-star-2 bg-star px-3 text-[13px] font-semibold text-ink hover:brightness-[1.04]"
              >
                添加模型 Key
              </Link>
            </div>
          ) : (
            <Select
              id={ids.key}
              value={key ? key.id : ''}
              aria-invalid={missingKey || undefined}
              onChange={(e) => {
                const k = keyList.find((x) => x.id === e.target.value)
                set('llm', { key_id: k?.id ?? null, model: k ? k.model : s.llm.model })
              }}
            >
              <option value="">先不选（小鱼不会回话）</option>
              {keyList.map((k) => (
                <option key={k.id} value={k.id}>
                  {k.name}（{k.model}）
                </option>
              ))}
            </Select>
          )}
        </Field>
        <ModelField
          key={key?.id}
          id={ids.model}
          value={s.llm.model}
          onChange={(model) => set('llm', { model })}
          models={provider?.models}
          fallback={key?.model}
        />
        <div className="grid gap-5">
          {fixedTemperature ? (
            <p className="text-sm text-ink-3">此模型使用固定温度。</p>
          ) : (
            <RangeField
              id={ids.temp}
              label="温度（Temperature）"
              hint="越低越稳定，越高越多样。"
              value={Math.min(s.llm.temperature, maxTemperature)}
              min={0}
              max={maxTemperature}
              step={0.1}
              format={(v) => v.toFixed(1)}
              onChange={(temperature) => set('llm', { temperature })}
            />
          )}
          <RangeField
            id={ids.turns}
            label="记住几轮对话"
            hint="记忆插件只负责长期记忆，近几轮对话仍要靠这里，设为 0 轮时小鱼记不住上一句话。"
            value={s.llm.context_turns}
            min={0}
            max={100}
            format={(v) => `${v} 轮`}
            onChange={(context_turns) => set('llm', { context_turns })}
          />
        </div>
        <Field label="单次回复最多 token" htmlFor={ids.tokens} error={tokensErr || undefined}>
          <Input
            id={ids.tokens}
            type="number"
            inputMode="numeric"
            min={1}
            max={32768}
            value={Number.isFinite(s.llm.max_tokens) ? s.llm.max_tokens : ''}
            aria-invalid={!!tokensErr || undefined}
            onChange={(e) => set('llm', { max_tokens: e.target.value === '' ? NaN : Number(e.target.value) })}
            className="tabular-nums sm:max-w-48"
          />
        </Field>
      </Block>

      <details className="group rounded-2xl border border-paper-3 bg-cream lg:col-span-2">
        <summary className="flex min-h-14 cursor-pointer list-none items-center justify-between gap-3 rounded-2xl px-5 font-display text-base font-black text-ink sm:px-6 [&::-webkit-details-marker]:hidden">
          回复规则
          <ChevronDown className="size-4 text-ink-3 group-open:rotate-180" aria-hidden />
        </summary>
        <div className="grid gap-5 px-5 pb-5 sm:px-6">
          <Field label="唤醒前缀" htmlFor={ids.prefixes} hint="例如 /help 中的 /。按回车或逗号添加。">
            <ChipsInput
              id={ids.prefixes}
              label="唤醒前缀"
              value={s.behavior.wake_prefixes}
              onChange={(wake_prefixes) => set('behavior', { wake_prefixes })}
              placeholder="/"
            />
          </Field>
          <div className="grid gap-4 rounded-xl border border-paper-3 bg-paper/50 p-4">
            <SwitchRow
              label="被 @ 时回复"
              checked={s.behavior.reply_when_at}
              onChange={(reply_when_at) => set('behavior', { reply_when_at })}
            />
            <SwitchRow
              label="私聊时回复"
              checked={s.behavior.reply_in_private}
              onChange={(reply_in_private) => set('behavior', { reply_in_private })}
            />
            <SwitchRow
              label="群里不 @ 也回复"
              hint="开启后可能增加回复次数和 token 用量。"
              checked={s.behavior.reply_in_group_without_at}
              onChange={(reply_in_group_without_at) => set('behavior', { reply_in_group_without_at })}
            />
          </div>
          <Field label="管理员" htmlFor={ids.admins} hint="可用管理指令。格式为「平台:账号」。">
            <ChipsInput
              id={ids.admins}
              label="管理员"
              value={s.behavior.admins}
              onChange={(admins) => set('behavior', { admins })}
              placeholder="qq:12345"
            />
          </Field>
          <Field label="黑名单" htmlFor={ids.block} hint="格式为「平台:账号」。">
            <ChipsInput
              id={ids.block}
              label="黑名单"
              value={s.behavior.blocklist}
              onChange={(blocklist) => set('behavior', { blocklist })}
              placeholder="qq:67890"
            />
          </Field>
          <Field label="每分钟回复上限" htmlFor={ids.rate} error={rateErr || undefined}>
            <Input
              id={ids.rate}
              type="number"
              inputMode="numeric"
              min={1}
              max={600}
              value={Number.isFinite(s.behavior.rate_limit_per_minute) ? s.behavior.rate_limit_per_minute : ''}
              aria-invalid={!!rateErr || undefined}
              onChange={(e) => set('behavior', { rate_limit_per_minute: e.target.value === '' ? NaN : Number(e.target.value) })}
              className="tabular-nums sm:max-w-48"
            />
          </Field>
        </div>
      </details>

      {dirty && (
        <div className="sticky bottom-[calc(4.75rem+env(safe-area-inset-bottom))] z-20 flex animate-[fade-in_150ms_ease-out] flex-wrap items-center gap-3 rounded-2xl border border-cobalt-900 bg-cobalt px-4 py-3 text-white shadow-[var(--shadow-lift)] lg:bottom-4 lg:col-span-2">
          <p className="min-w-0 flex-1 text-sm font-semibold">{invalid ? '有几项没填对，改好才能保存' : '改动还没保存'}</p>
          <Button variant="ghost" size="sm" onClick={() => setDraft(null)} className="text-white/85 hover:bg-white/10 hover:text-white">
            <Undo2 aria-hidden />
            放弃修改
          </Button>
          <Button type="submit" variant="primary" size="sm" loading={save.isPending} disabled={invalid}>
            保存
          </Button>
        </div>
      )}
    </form>
  )
}
