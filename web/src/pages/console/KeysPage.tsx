import { useRef, useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ExternalLink, Plus, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api, ApiError } from '@/api/client'
import { keys, useBots, useLlmKeys } from '@/api/hooks'
import { modelProviders, providerForEndpoint } from '@/api/modelProviders'
import type { LlmKey } from '@/api/types'
import { Button, ConfirmDialog, ErrorNote, Field, Input, PageHeader, Skeleton, useFieldId } from '@/components/ui'
import { cn } from '@/lib/cn'
import { fmtDate } from '@/lib/format'
import { Select } from './bot/shared'
import ModelField from './ModelField'

type Errors = Partial<Record<'provider' | 'name' | 'base_url' | 'model' | 'api_key' | 'form', string>>

function ProviderPlatformLink({ provider }: { provider: (typeof modelProviders)[number] }) {
  return (
    <a href={provider.platform_url} target="_blank" rel="noreferrer" title={`${provider.name} 官方平台`}
      className="inline-flex min-w-0 items-center gap-1 text-[13px] font-semibold text-sky underline-offset-2 hover:underline">
      <span className="wrap-anywhere">{new URL(provider.platform_url).hostname}</span>
      <ExternalLink className="size-3.5 shrink-0" aria-hidden />
    </a>
  )
}

function AddKeyForm({ id, onSaved, onCancel }: { id: string; onSaved: () => void; onCancel?: () => void }) {
  const qc = useQueryClient()
  const [provider, setProvider] = useState('')
  const [name, setName] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [model, setModel] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [showConnection, setShowConnection] = useState(false)
  const [busy, setBusy] = useState(false)
  const [errors, setErrors] = useState<Errors>({})
  const ids = {
    provider: useFieldId('key-provider'),
    name: useFieldId('key-name'),
    base: useFieldId('key-base'),
    model: useFieldId('key-model'),
    key: useFieldId('key-secret'),
  }
  const preset = modelProviders.find((p) => p.id === provider)
  const connectionOpen = !!provider && (!preset || showConnection)

  function pick(value: string) {
    const next = modelProviders.find((p) => p.id === value)
    setProvider(value)
    setBaseUrl(next?.base_url ?? '')
    setModel(next?.models[0] ?? '')
    if (!name.trim() || name === preset?.name) setName(next?.name ?? '')
    setShowConnection(false)
    setErrors({})
  }

  async function submit(e: FormEvent) {
    e.preventDefault()
    const next: Errors = {}
    if (!provider) next.provider = '请选择服务商'
    if (provider) {
      if (!name.trim()) next.name = '请输入名称'
      if (!/^https?:\/\/\S+$/.test(baseUrl.trim())) next.base_url = '请输入完整的 HTTPS 地址'
      if (!model.trim()) next.model = '请输入服务商的模型名'
    }
    if (!apiKey.trim()) next.api_key = '请输入 API Key'
    setErrors(next)
    if (next.name || next.base_url || next.model) setShowConnection(true)
    if (Object.keys(next).length) return
    setBusy(true)
    try {
      const k = await api.addLlmKey({ name: name.trim(), base_url: baseUrl.trim().replace(/\/+$/, ''), model: model.trim(), api_key: apiKey.trim() })
      qc.setQueryData<LlmKey[]>(keys.llmKeys, (old) => [...(old ?? []).filter((item) => item.id !== k.id), k])
      qc.invalidateQueries({ queryKey: keys.llmKeys })
      toast.success(`${k.name} 已保存`)
      setApiKey('')
      onSaved()
    } catch (err) {
      if (err instanceof ApiError && err.code === 'llm_key_invalid') setErrors({ api_key: err.message })
      else setErrors({ form: err instanceof Error ? err.message : '没能保存，稍后再试' })
    } finally {
      setBusy(false)
    }
  }

  return (
    <form id={id} aria-labelledby={`${id}-title`} onSubmit={submit} noValidate className="grid gap-4 rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <h2 id={`${id}-title`} className="text-lg font-semibold text-ink">添加 Key</h2>
      <Field label="服务商" htmlFor={ids.provider} error={errors.provider} aside={preset && <ProviderPlatformLink provider={preset} />} hint={
        ['openai', 'google', 'anthropic'].includes(provider) ? '中国大陆未列入此接口的支持地区。' : undefined
      }>
        <Select id={ids.provider} value={provider} onChange={(e) => pick(e.target.value)} autoFocus={!!onCancel} disabled={busy} aria-invalid={!!errors.provider} required>
          <option value="" disabled>选择服务商</option>
          {modelProviders.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          <option value="custom">自定义 OpenAI 兼容接口</option>
        </Select>
      </Field>
      {provider && <ModelField key={provider} id={ids.model} value={model} onChange={setModel} models={preset?.models} error={errors.model} disabled={busy} />}
      {preset && (
        <button
          type="button"
          aria-expanded={connectionOpen}
          aria-controls={`${id}-connection`}
          onClick={() => setShowConnection(!showConnection)}
          className="-my-1 flex w-full cursor-pointer items-center gap-2 rounded-lg py-2 text-left text-sm font-semibold text-ink-2 hover:text-sky"
        >
          <ChevronDown aria-hidden className={cn('size-4 shrink-0', !connectionOpen && '-rotate-90')} />
          <span className="shrink-0">连接设置</span>
        </button>
      )}
      <div id={`${id}-connection`} hidden={!connectionOpen} className={cn(connectionOpen && 'grid gap-4 border-t border-paper-3 pt-4')}>
        <Field label="名称" htmlFor={ids.name} error={errors.name}>
          <Input id={ids.name} value={name} onChange={(e) => setName(e.target.value)} maxLength={40} aria-invalid={!!errors.name} disabled={busy} />
        </Field>
        <Field label="接口地址" htmlFor={ids.base} error={errors.base_url}>
          <Input
            id={ids.base}
            type="url"
            inputMode="url"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="https://api.deepseek.com"
            autoCapitalize="off"
            spellCheck={false}
            aria-invalid={!!errors.base_url}
            disabled={busy}
          />
        </Field>
      </div>
      <Field label="API Key" htmlFor={ids.key} error={errors.api_key} hint="向服务商验证后加密保存，仅显示部分字符。">
        <Input
          id={ids.key}
          type="password"
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          placeholder="sk-..."
          autoComplete="off"
          spellCheck={false}
          aria-invalid={!!errors.api_key}
          disabled={busy}
          className="font-mono text-sm"
        />
      </Field>
      {errors.form && (
        <p role="alert" className="rounded-xl bg-bad-soft px-3.5 py-2.5 text-sm text-bad">
          {errors.form}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant="primary" loading={busy}>
          {busy ? '正在验证' : '验证并保存'}
        </Button>
        {onCancel && <Button variant="ghost" disabled={busy} onClick={onCancel}>取消</Button>}
      </div>
    </form>
  )
}

function KeyRow({ k, usedBy }: { k: LlmKey; usedBy: string[] }) {
  const qc = useQueryClient()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const provider = providerForEndpoint(k.base_url)

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
    <li className="flex items-start gap-3 py-4">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5">
          <h3 className="font-semibold wrap-anywhere text-ink">{k.name}</h3>
          {provider && <ProviderPlatformLink provider={provider} />}
        </div>
        <p className="mt-1 flex flex-wrap items-baseline gap-x-3 gap-y-0.5 break-all text-sm text-ink-2">
          <span>{k.model}</span>
          <code className="text-ink-3">{k.key_masked}</code>
          {k.via_gateway && (
            <span className="text-ink-3" title="托管服务器在国内，请求经 Cloudflare AI Gateway 转发到服务商">
              经 AI Gateway 转发
            </span>
          )}
        </p>
        {usedBy.length > 0 && <p className="mt-1 break-words text-sm text-ink-3">使用中 · {usedBy.join('、')}</p>}
        <details className="mt-1 text-sm text-ink-3">
          <summary aria-label={`${k.name} 的详情`} className="w-fit cursor-pointer rounded-md py-1 font-semibold text-ink-2 hover:text-sky">详情</summary>
          <p className="break-all">{k.base_url}</p>
          <p className="mt-0.5 tabular-nums">{fmtDate(k.created_at)} 添加</p>
        </details>
      </div>
      <Button variant="ghost" className="size-11 px-0" onClick={() => setOpen(true)} aria-label={`删除 ${k.name}`} title={`删除 ${k.name}`}>
        <Trash2 aria-hidden />
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={`删除 ${k.name}？`}
        body={
          usedBy.length > 0 ? (
            <>
              {usedBy.join('、')} 正在使用。删除后，需在「机器人设置」中换一个 Key 才能继续回复。
            </>
          ) : (
            '删除后如需使用，要重新添加。'
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
  const [adding, setAdding] = useState(false)
  const addButton = useRef<HTMLButtonElement>(null)
  const formId = useFieldId('add-key')
  const items = list.data ?? []
  const empty = list.isSuccess && items.length === 0

  function closeForm() {
    setAdding(false)
    requestAnimationFrame(() => addButton.current?.focus())
  }

  return (
    <div className="max-w-2xl">
      <PageHeader
        title="模型 Key"
        description="添加后在「机器人设置」中选择。"
        actions={!list.isLoading && !empty && !adding && (
          <Button ref={addButton} variant="primary" onClick={() => setAdding(true)}>
            <Plus aria-hidden />
            添加 Key
          </Button>
        )}
      />
      <div className="grid gap-5">
        {(empty || adding) && <AddKeyForm id={formId} onSaved={closeForm} onCancel={adding && !empty ? closeForm : undefined} />}
        {list.isLoading && <Skeleton className="h-24" />}
        {list.error && <ErrorNote error={list.error} onRetry={() => list.refetch()} />}
        {items.length > 0 && (
          <section aria-labelledby="keys-list">
            <h2 id="keys-list" className="text-base font-semibold text-ink">已保存</h2>
            <ul className="mt-1 divide-y divide-paper-3 border-y border-paper-3">
              {items.map((k) => (
                <KeyRow key={k.id} k={k} usedBy={(bots.data ?? []).filter((b) => b.settings.llm.key_id === k.id).map((b) => b.name)} />
              ))}
            </ul>
          </section>
        )}
      </div>
    </div>
  )
}
