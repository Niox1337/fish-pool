import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Lock, Plus, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api, ApiError } from '@/api/client'
import { keys, useBots, useLlmKeys } from '@/api/hooks'
import type { LlmKey } from '@/api/types'
import { Button, ConfirmDialog, ErrorNote, Field, Input, PageHeader, Skeleton, useFieldId } from '@/components/ui'
import { KeySolid } from '@/components/icons'
import { cn } from '@/lib/cn'
import { fmtDate } from '@/lib/format'

const presets = [
  { name: 'DeepSeek', base_url: 'https://api.deepseek.com/v1', model: 'deepseek-chat' },
  { name: 'OpenAI', base_url: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
  { name: 'SiliconFlow', base_url: 'https://api.siliconflow.cn/v1', model: 'deepseek-ai/DeepSeek-V3' },
  { name: 'Moonshot', base_url: 'https://api.moonshot.cn/v1', model: 'moonshot-v1-8k' },
  { name: '智谱', base_url: 'https://open.bigmodel.cn/api/paas/v4', model: 'glm-4-flash' },
]

type Errors = Partial<Record<'name' | 'base_url' | 'model' | 'api_key' | 'form', string>>

function AddKeyForm({ first }: { first: boolean }) {
  const qc = useQueryClient()
  const [name, setName] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [model, setModel] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [errors, setErrors] = useState<Errors>({})
  const ids = {
    name: useFieldId('key-name'),
    base: useFieldId('key-base'),
    model: useFieldId('key-model'),
    key: useFieldId('key-secret'),
  }

  function pick(p: (typeof presets)[number]) {
    setBaseUrl(p.base_url)
    if (!model) setModel(p.model)
    if (!name) setName(p.name)
    setErrors({})
  }

  async function submit(e: FormEvent) {
    e.preventDefault()
    const next: Errors = {}
    if (!name.trim()) next.name = '起个名字，方便在小鱼设置里认出它'
    if (!/^https?:\/\/\S+$/.test(baseUrl.trim())) next.base_url = '填完整的地址，以 https:// 开头'
    if (!model.trim()) next.model = '填一个模型名，比如 deepseek-chat'
    if (!apiKey.trim()) next.api_key = '把服务商给你的 Key 粘贴进来'
    setErrors(next)
    if (Object.keys(next).length) return
    setBusy(true)
    try {
      const k = await api.addLlmKey({ name: name.trim(), base_url: baseUrl.trim().replace(/\/+$/, ''), model: model.trim(), api_key: apiKey.trim() })
      qc.setQueryData<LlmKey[]>(keys.llmKeys, (old) => [...(old ?? []), k])
      qc.invalidateQueries({ queryKey: keys.llmKeys })
      toast.success(`${k.name} 已保存，可以在小鱼的人设与模型里选它了`)
      setName('')
      setBaseUrl('')
      setModel('')
      setApiKey('')
    } catch (err) {
      if (err instanceof ApiError && err.code === 'llm_key_invalid') setErrors({ api_key: `服务商没有认这个 Key：${err.message}` })
      else setErrors({ form: err instanceof Error ? err.message : '没能保存，稍后再试' })
    } finally {
      setBusy(false)
    }
  }

  return (
    <form onSubmit={submit} noValidate className="grid gap-4 rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <h2 className="font-display text-lg font-black text-ink">{first ? '填上第一个 Key' : '再加一个 Key'}</h2>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="名字" htmlFor={ids.name} error={errors.name}>
          <Input id={ids.name} value={name} onChange={(e) => setName(e.target.value)} placeholder="我的 DeepSeek" maxLength={40} aria-invalid={!!errors.name} />
        </Field>
        <Field label="模型" htmlFor={ids.model} error={errors.model} hint="小鱼默认用这个模型，之后也能在每条小鱼里改。">
          <Input
            id={ids.model}
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="deepseek-chat"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            aria-invalid={!!errors.model}
          />
        </Field>
      </div>
      <Field
        label="接口地址"
        htmlFor={ids.base}
        error={errors.base_url}
        hint="需要兼容 OpenAI 接口的地址，一般以 /v1 结尾。点下面的服务商可以直接填好。"
      >
        <Input
          id={ids.base}
          type="url"
          inputMode="url"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder="https://api.deepseek.com/v1"
          autoCapitalize="off"
          spellCheck={false}
          aria-invalid={!!errors.base_url}
          className="font-mono text-sm"
        />
      </Field>
      <div className="-mt-1 flex flex-wrap gap-1.5" role="group" aria-label="常用服务商">
        {presets.map((p) => (
          <button
            key={p.name}
            type="button"
            onClick={() => pick(p)}
            aria-pressed={baseUrl === p.base_url}
            className={cn(
              'cursor-pointer rounded-full border px-3 py-1 text-[13px] font-semibold transition-colors duration-150',
              baseUrl === p.base_url ? 'border-cobalt bg-cobalt text-white' : 'border-paper-3 bg-white text-ink-2 hover:border-ink-3/50 hover:text-ink',
            )}
          >
            {p.name}
          </button>
        ))}
      </div>
      <Field label="API Key" htmlFor={ids.key} error={errors.api_key} hint="保存时会先用它请求一次模型列表，确认能用才存下。">
        <Input
          id={ids.key}
          type="password"
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          placeholder="sk-..."
          autoComplete="off"
          spellCheck={false}
          aria-invalid={!!errors.api_key}
          className="font-mono text-sm"
        />
      </Field>
      {errors.form && (
        <p role="alert" className="rounded-xl bg-bad-soft px-3.5 py-2.5 text-sm text-bad">
          {errors.form}
        </p>
      )}
      <div>
        <Button type="submit" variant="primary" loading={busy}>
          <Plus aria-hidden />
          检查并保存
        </Button>
      </div>
    </form>
  )
}

function KeyRow({ k, usedBy }: { k: LlmKey; usedBy: string[] }) {
  const qc = useQueryClient()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)

  async function remove() {
    setBusy(true)
    try {
      await api.deleteLlmKey(k.id)
      qc.setQueryData<LlmKey[]>(keys.llmKeys, (old) => old?.filter((x) => x.id !== k.id))
      qc.invalidateQueries({ queryKey: keys.llmKeys })
      toast.success(`${k.name} 已删除`)
      setOpen(false)
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '没能删除，稍后再试')
    } finally {
      setBusy(false)
    }
  }

  return (
    <li className="flex flex-wrap items-start gap-x-4 gap-y-2 py-4 first:pt-0 last:pb-0">
      <KeySolid className="mt-1 size-5 shrink-0 text-star-2" />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5">
          <h3 className="font-semibold text-ink">{k.name}</h3>
          <span className="font-mono text-[13px] text-ink-2">{k.model}</span>
        </div>
        <p className="mt-0.5 truncate font-mono text-[13px] text-ink-3" title={k.base_url}>
          {k.base_url}
        </p>
        <p className="mt-1 flex flex-wrap gap-x-4 text-[13px] text-ink-3">
          <span className="font-mono tracking-wide text-ink-2">{k.key_masked}</span>
          <span className="tabular-nums">{fmtDate(k.created_at)} 添加</span>
          {usedBy.length > 0 && <span>{usedBy.join('、')} 在用</span>}
        </p>
      </div>
      <Button size="sm" variant="ghost" onClick={() => setOpen(true)} aria-label={`删除 ${k.name}`}>
        <Trash2 aria-hidden />
        删除
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={`删除 ${k.name}？`}
        body={
          usedBy.length > 0 ? (
            <>
              {usedBy.join('、')} 正在用这个 Key，删除后它们会说不出话，直到你在人设与模型里换一个 Key。
            </>
          ) : (
            '删除后托管班不再保存这个 Key，想再用需要重新填写。'
          )
        }
        confirmLabel="删除 Key"
        onConfirm={remove}
        loading={busy}
      />
    </li>
  )
}

export default function KeysPage() {
  const list = useLlmKeys()
  const bots = useBots()
  const items = list.data ?? []

  return (
    <>
      <PageHeader
        title="模型 Key"
        description="小鱼说话要用你自己的大模型 Key，托管班不提供模型额度。任何兼容 OpenAI 接口的服务商都可以。"
      />
      <p className="mb-6 flex items-start gap-2 rounded-2xl bg-sky-soft px-4 py-3 text-sm leading-relaxed text-ink-2">
        <Lock className="mt-0.5 size-4 shrink-0 text-sky" aria-hidden />
        Key 加密后保存，之后只显示最后几位，管理员也看不到完整的 Key。
      </p>

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)] lg:items-start">
        <section aria-labelledby="keys-list">
          <h2 id="keys-list" className="mb-3 font-display text-lg font-black text-ink">
            已保存的 Key
          </h2>
          {list.isLoading ? (
            <div className="grid gap-3">
              <Skeleton className="h-20" />
              <Skeleton className="h-20" />
            </div>
          ) : list.error ? (
            <ErrorNote error={list.error} onRetry={() => list.refetch()} />
          ) : items.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-paper-3 p-5 text-[15px] leading-relaxed text-ink-2">
              <p className="font-semibold text-ink">还没有 Key</p>
              <p className="mt-1">去服务商的控制台创建一个 API Key，比如 DeepSeek 开放平台，再粘贴到旁边的表单里。保存后在小鱼的人设与模型里选它。</p>
            </div>
          ) : (
            <ul className="divide-y divide-paper-3 rounded-2xl border border-paper-3 bg-cream p-5">
              {items.map((k) => (
                <KeyRow key={k.id} k={k} usedBy={(bots.data ?? []).filter((b) => b.settings.llm.key_id === k.id).map((b) => b.name)} />
              ))}
            </ul>
          )}
        </section>
        <AddKeyForm first={!list.isLoading && items.length === 0} />
      </div>
    </>
  )
}
