import { useEffect, type ReactNode } from 'react'
import { Link } from 'react-router'
import { KeyRound, Undo2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { useBotMutation, useLlmKeys } from '@/api/hooks'
import type { Bot, BotSettings } from '@/api/types'
import { Button, Field, Input, Skeleton, Textarea, useFieldId } from '@/components/ui'
import { ChipsInput, RangeField, Select, SwitchRow } from './shared'
import { useErrorToast } from './errors'

const PROMPT_SOFT_LIMIT = 4000

function Block({ title, intro, children }: { title: string; intro?: ReactNode; children: ReactNode }) {
  return (
    <section className="rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <h2 className="font-display text-lg font-black text-ink">{title}</h2>
      {intro && <p className="mt-1 text-sm leading-relaxed text-ink-2">{intro}</p>}
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

  useEffect(() => {
    if (!dirty) return
    const warn = (e: BeforeUnloadEvent) => e.preventDefault()
    window.addEventListener('beforeunload', warn)
    return () => window.removeEventListener('beforeunload', warn)
  }, [dirty])

  function set<K extends keyof BotSettings>(k: K, patch: Partial<BotSettings[K]>) {
    setDraft({ ...s, [k]: { ...s[k], ...patch } })
  }

  const keyList = llmKeys.data ?? []
  const key = keyList.find((k) => k.id === s.llm.key_id)
  const missingKey = !!s.llm.key_id && llmKeys.isSuccess && !key
  const tokensErr = intOk(s.llm.max_tokens, 64, 32768) ? '' : '填 64 到 32768 之间的整数'
  const rateErr = intOk(s.behavior.rate_limit_per_minute, 0, 600) ? '' : '填 0 到 600 之间的整数'
  const nameErr = s.persona.name.trim() ? '' : '给人设起个名字'
  const invalid = !!(tokensErr || rateErr || nameErr)
  const promptLen = [...s.persona.system_prompt].length

  function submit() {
    if (invalid) return
    save.mutate(s, {
      onSuccess: () => {
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
      className="grid gap-4"
    >
      <Block title="人设" intro="小鱼是谁、怎么说话，都写在这里。">
        <Field label="名字" htmlFor={ids.name} error={nameErr || undefined} hint="小鱼自称和被人提到时用的名字。">
          <Input
            id={ids.name}
            value={s.persona.name}
            maxLength={24}
            aria-invalid={!!nameErr || undefined}
            onChange={(e) => set('persona', { name: e.target.value })}
          />
        </Field>
        <Field
          label="系统提示词"
          htmlFor={ids.prompt}
          hint="写清楚它的性格、口头禅、该做什么不该做什么。越具体越像。"
          aside={
            <span className={promptLen > PROMPT_SOFT_LIMIT ? 'text-[13px] font-semibold text-warn tabular-nums' : 'text-[13px] text-ink-3 tabular-nums'}>
              {promptLen} 字{promptLen > PROMPT_SOFT_LIMIT ? '，有点长了，会多花 token' : ''}
            </span>
          }
        >
          <Textarea id={ids.prompt} rows={8} value={s.persona.system_prompt} onChange={(e) => set('persona', { system_prompt: e.target.value })} />
        </Field>
        <Field label="开场白" htmlFor={ids.greeting} hint="新会话里小鱼主动说的第一句话。留空就不主动打招呼。">
          <Input
            id={ids.greeting}
            value={s.persona.greeting}
            placeholder="大家好，我是新来的小鱼"
            onChange={(e) => set('persona', { greeting: e.target.value })}
          />
        </Field>
      </Block>

      <Block title="模型" intro="小鱼用你在模型 Key 页添加的 Key 说话。">
        <Field
          label="模型 Key"
          htmlFor={ids.key}
          error={missingKey ? '之前选的 Key 已经删掉了，请重新选一个' : undefined}
          aside={
            <Link to="/app/keys" className="text-[13px] font-semibold text-sky hover:underline">
              管理 Key
            </Link>
          }
        >
          {llmKeys.isLoading ? (
            <Skeleton className="h-11" />
          ) : keyList.length === 0 ? (
            <div className="flex flex-wrap items-center gap-3 rounded-xl border border-dashed border-paper-3 bg-paper/60 px-4 py-3">
              <KeyRound className="size-5 shrink-0 text-ink-3" aria-hidden />
              <p className="min-w-0 flex-1 basis-48 text-sm text-ink-2">还没有模型 Key。添加一个以后回来这里选上，小鱼就能开口了。</p>
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
        <Field label="模型名" htmlFor={ids.model} hint={key ? `选了 Key 会自动填上 ${key.model}，也可以换成同一家的其他模型。` : '和 Key 对应服务商的模型名一致。'}>
          <Input
            id={ids.model}
            value={s.llm.model}
            placeholder={key?.model ?? 'deepseek-chat'}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => set('llm', { model: e.target.value })}
            className="font-mono"
          />
        </Field>
        <div className="grid gap-5 sm:grid-cols-2">
          <RangeField
            id={ids.temp}
            label="温度"
            hint="越低回答越稳，越高越活泼跳脱。"
            value={s.llm.temperature}
            min={0}
            max={2}
            step={0.1}
            format={(v) => v.toFixed(1)}
            onChange={(temperature) => set('llm', { temperature })}
          />
          <RangeField
            id={ids.turns}
            label="记住几轮对话"
            hint="越多越懂上下文，也越费 token。"
            value={s.llm.context_turns}
            min={0}
            max={50}
            format={(v) => `${v} 轮`}
            onChange={(context_turns) => set('llm', { context_turns })}
          />
        </div>
        <Field label="单次回复最多 token" htmlFor={ids.tokens} error={tokensErr || undefined} hint="限制每条回复的长度，1024 大约是几百个汉字。">
          <Input
            id={ids.tokens}
            type="number"
            inputMode="numeric"
            min={64}
            max={32768}
            step={64}
            value={Number.isFinite(s.llm.max_tokens) ? s.llm.max_tokens : ''}
            aria-invalid={!!tokensErr || undefined}
            onChange={(e) => set('llm', { max_tokens: e.target.value === '' ? NaN : Number(e.target.value) })}
            className="tabular-nums sm:max-w-48"
          />
        </Field>
      </Block>

      <Block title="回复规则" intro="决定小鱼什么时候开口、听谁的话。">
        <Field label="唤醒前缀" htmlFor={ids.prefixes} hint="消息以这些字符开头时当作指令，比如 /help。按回车或逗号添加。">
          <ChipsInput id={ids.prefixes} label="唤醒前缀" value={s.behavior.wake_prefixes} onChange={(wake_prefixes) => set('behavior', { wake_prefixes })} placeholder="/" />
        </Field>
        <div className="grid gap-4 rounded-xl border border-paper-3 bg-paper/50 p-4">
          <SwitchRow label="被 @ 时回复" checked={s.behavior.reply_when_at} onChange={(reply_when_at) => set('behavior', { reply_when_at })} />
          <SwitchRow label="私聊时回复" checked={s.behavior.reply_in_private} onChange={(reply_in_private) => set('behavior', { reply_in_private })} />
          <SwitchRow
            label="群里不 @ 也回复"
            hint="打开后群里每条消息小鱼都可能接话，比较吵，也更费 token。"
            checked={s.behavior.reply_in_group_without_at}
            onChange={(reply_in_group_without_at) => set('behavior', { reply_in_group_without_at })}
          />
        </div>
        <Field label="管理员" htmlFor={ids.admins} hint="管理员能用管理指令。写成 平台:账号，比如 qq:12345。">
          <ChipsInput id={ids.admins} label="管理员" value={s.behavior.admins} onChange={(admins) => set('behavior', { admins })} placeholder="qq:12345" />
        </Field>
        <Field label="黑名单" htmlFor={ids.block} hint="小鱼不理这些账号的消息，格式同上。">
          <ChipsInput id={ids.block} label="黑名单" value={s.behavior.blocklist} onChange={(blocklist) => set('behavior', { blocklist })} placeholder="qq:67890" />
        </Field>
        <Field label="每分钟最多回复几条" htmlFor={ids.rate} error={rateErr || undefined} hint="防止被刷屏，0 表示不限制。">
          <Input
            id={ids.rate}
            type="number"
            inputMode="numeric"
            min={0}
            max={600}
            value={Number.isFinite(s.behavior.rate_limit_per_minute) ? s.behavior.rate_limit_per_minute : ''}
            aria-invalid={!!rateErr || undefined}
            onChange={(e) => set('behavior', { rate_limit_per_minute: e.target.value === '' ? NaN : Number(e.target.value) })}
            className="tabular-nums sm:max-w-48"
          />
        </Field>
      </Block>

      {dirty && (
        <div className="sticky bottom-[calc(4.75rem+env(safe-area-inset-bottom))] z-20 flex animate-[fade-in_150ms_ease-out] flex-wrap items-center gap-3 rounded-2xl border border-cobalt-900 bg-cobalt px-4 py-3 text-white shadow-[var(--shadow-lift)] lg:bottom-4">
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
