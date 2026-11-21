import { useEffect, useId, useRef, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'
import { ExternalLink, Plus, RotateCw, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import type { Bot, Platform, QqPanel, QqPanelItem, QqPanelRecord, QqPanelScope } from '@/api/types'
import { Button, ConfirmDialog, ErrorNote, Field, Input, Sheet, Skeleton } from '@/components/ui'
import { ActionError, Select, SwitchRow } from './shared'

const DOCS = 'https://bot.q.qq.com/wiki/develop/api-v2/server-inter/menu-panel/'
const SCENES = { group: '群聊', c2c: '单聊' } as const
const newItem = (): QqPanelItem => ({ type: 'command', name: '', desc: '' })
const count = (text: string) => [...text].length

function panelItemError(item: QqPanelItem) {
  const name = item.name.trim()
  if (!name || count(name) > 14) return '请输入 1 到 14 个字符（约 7 个汉字）。'
  if (count(item.desc ?? '') > 30) return '说明最多 30 个字符（约 15 个汉字）。'
  if (item.type === 'link') {
    try {
      const url = new URL(item.link ?? '')
      if (url.protocol === 'https:' && !url.username && !url.password) return ''
    } catch {
      return '请输入有效的 HTTPS 地址。'
    }
    return '请输入有效的 HTTPS 地址。'
  }
  return ''
}

function PanelEditor({
  botId,
  platformId,
  scope,
  record,
  onClose,
  onSaved,
  returnFocus,
}: {
  botId: string
  platformId: string
  scope: QqPanelScope
  record: QqPanelRecord | null
  onClose: () => void
  onSaved: () => void
  returnFocus: () => void
}) {
  const fieldId = useId()
  const [remark, setRemark] = useState(record?.panel?.remark ?? '')
  const [rows, setRows] = useState(() => (record?.panel?.items ?? [newItem()]).map((item) => ({ id: crypto.randomUUID(), item })))
  const [discard, setDiscard] = useState(false)
  const [checked, setChecked] = useState(false)
  const [unsaved, setUnsaved] = useState(false)
  const save = useMutation<unknown, Error, QqPanel>({
    mutationFn: (panel: QqPanel) =>
      record
        ? api.updateQqPanel(botId, platformId, record.panel_id, panel, record.version ?? record.panel?.version)
        : api.createQqPanel(botId, platformId, scope, panel),
    onSuccess: () => {
      toast.success('指令面板已提交给 QQ')
      onSaved()
    },
  })
  const errors = rows.map(({ item }) => panelItemError(item))
  const remarkError = count(remark) > 255 ? '备注最多 255 个字符。' : ''
  const isValid = (record !== null || rows.length > 0) && !remarkError && !errors.some(Boolean)

  useEffect(() => {
    if (!unsaved) return
    const warn = (event: BeforeUnloadEvent) => event.preventDefault()
    window.addEventListener('beforeunload', warn)
    return () => window.removeEventListener('beforeunload', warn)
  }, [unsaved])

  function close() {
    if (save.isPending) return
    if (unsaved) setDiscard(true)
    else onClose()
  }

  function changeRow(id: string, patch: Partial<QqPanelItem>) {
    setUnsaved(true)
    setRows((current) => current.map((row) => (row.id === id ? { ...row, item: { ...row.item, ...patch } } : row)))
  }

  return (
    <Sheet
      open
      side="right"
      returnFocus={returnFocus}
      title={record ? '编辑指令面板' : '新建指令面板'}
      description={`${SCENES[scope]} · ${record?.target_type === 'specific' ? '指定会话' : '全部会话'}`}
      onOpenChange={(open) => {
        if (!open) close()
      }}
    >
      <form
        className="flex h-full min-h-0 flex-col"
        noValidate
        onSubmit={(event) => {
          event.preventDefault()
          if (save.isPending || discard) return
          setChecked(true)
          if (isValid) save.mutate({ items: rows.map(({ item }) => item), remark })
        }}
      >
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 py-5 sm:px-8 sm:py-6">
          <fieldset disabled={save.isPending} className="grid min-w-0 gap-5">
            <Field label="备注" htmlFor={`${fieldId}-remark`} error={remarkError || undefined}>
              <Input
                id={`${fieldId}-remark`}
                placeholder="仅自己可见"
                value={remark}
                onChange={(event) => {
                  setRemark(event.target.value)
                  setUnsaved(true)
                }}
              />
            </Field>
            <ol className="divide-y divide-paper-3 border-y border-paper-3">
              {rows.map(({ id, item }, i) => (
                <li key={id} className="grid gap-3 py-4">
                  <div className="flex items-center gap-2">
                    <Select
                      aria-label={`第 ${i + 1} 项类型`}
                      value={item.type}
                      className="max-w-48 flex-1"
                      onChange={(event) => changeRow(id, { type: event.target.value === 'link' ? 'link' : 'command' })}
                    >
                      <option value="command">指令</option>
                      <option value="link">链接</option>
                    </Select>
                    <Button
                      variant="ghost"
                      className="ml-auto size-11 px-0"
                      aria-label={`移除第 ${i + 1} 项`}
                      onClick={() => {
                        setRows((current) => current.filter((row) => row.id !== id))
                        setUnsaved(true)
                      }}
                    >
                      <Trash2 aria-hidden />
                    </Button>
                  </div>
                  <Field htmlFor={`${id}-name`} label={item.type === 'command' ? '指令内容' : '链接名称'}>
                    <Input
                      id={`${id}-name`}
                      value={item.name}
                      placeholder={item.type === 'command' ? '/帮助' : '使用文档'}
                      onChange={(event) => changeRow(id, { name: event.target.value })}
                    />
                  </Field>
                  {item.type === 'link' && (
                    <Field htmlFor={`${id}-link`} label="跳转地址">
                      <Input
                        id={`${id}-link`}
                        type="url"
                        inputMode="url"
                        value={item.link ?? ''}
                        placeholder="https://"
                        onChange={(event) => changeRow(id, { link: event.target.value })}
                      />
                    </Field>
                  )}
                  <Field htmlFor={`${id}-desc`} label="说明">
                    <Input
                      id={`${id}-desc`}
                      value={item.desc ?? ''}
                      placeholder="选填，展示给用户"
                      onChange={(event) => changeRow(id, { desc: event.target.value })}
                    />
                  </Field>
                  {scope === 'group' && (
                    <SwitchRow
                      label="仅群管理员可点"
                      checked={!!item.only_admin}
                      onChange={(only_admin) => changeRow(id, { only_admin })}
                    />
                  )}
                  {checked && errors[i] && (
                    <p role="alert" className="text-sm text-bad">
                      {errors[i]}
                    </p>
                  )}
                </li>
              ))}
            </ol>
            <Button
              variant="secondary"
              className="justify-self-start"
              disabled={rows.length >= 20}
              onClick={() => {
                setRows((current) => [...current, { id: crypto.randomUUID(), item: newItem() }])
                setUnsaved(true)
              }}
            >
              <Plus aria-hidden /> 添加一项
            </Button>
            {checked && !record && rows.length === 0 && (
              <p role="alert" className="text-sm text-bad">
                请添加至少一项。
              </p>
            )}
          </fieldset>
        </div>
        <footer className="grid shrink-0 gap-2 border-t border-paper-3 bg-cream px-5 pt-4 pb-[max(1rem,env(safe-area-inset-bottom))] sm:px-8">
          {save.error && <ActionError error={save.error} />}
          {discard ? (
            <div className="flex flex-wrap items-center justify-end gap-2">
              <p className="mr-auto text-sm text-ink-2">放弃未发布的修改？</p>
              <Button variant="ghost" onClick={() => setDiscard(false)}>
                继续编辑
              </Button>
              <Button variant="danger" onClick={onClose}>
                放弃
              </Button>
            </div>
          ) : (
            <div className="flex justify-end gap-2">
              <Button variant="ghost" disabled={save.isPending} onClick={close}>
                取消
              </Button>
              <Button type="submit" variant="primary" loading={save.isPending} disabled={!!record && !unsaved}>
                发布到 QQ
              </Button>
            </div>
          )}
        </footer>
      </form>
    </Sheet>
  )
}

function PanelList({ bot, platform }: { bot: Bot; platform: Platform }) {
  const sceneId = useId()
  const [scope, setScope] = useState<QqPanelScope>('group')
  const [editor, setEditor] = useState<{ record: QqPanelRecord | null } | null>(null)
  const [deleting, setDeleting] = useState<QqPanelRecord | null>(null)
  const trigger = useRef<HTMLButtonElement | null>(null)
  const panels = useQuery({
    queryKey: ['qq-panels', bot.id, platform.id, scope],
    queryFn: () => api.qqPanels(bot.id, platform.id, scope),
    retry: false,
    staleTime: 30_000,
  })
  const remove = useMutation({
    mutationFn: (record: QqPanelRecord) => api.deleteQqPanel(bot.id, platform.id, record.panel_id),
    onSuccess: () => {
      toast.success('已从 QQ 删除面板')
      setDeleting(null)
      void panels.refetch()
    },
  })
  const records = panels.data?.records ?? []
  const focusTrigger = () => trigger.current?.focus()

  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <Field label="生效场景" htmlFor={sceneId} className="min-w-28">
          <Select id={sceneId} value={scope} onChange={(event) => setScope(event.target.value === 'c2c' ? 'c2c' : 'group')}>
            <option value="group">群聊</option>
            <option value="c2c">单聊</option>
          </Select>
        </Field>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            variant="ghost"
            loading={panels.isFetching}
            onClick={() => {
              void panels.refetch()
            }}
          >
            <RotateCw aria-hidden /> 刷新
          </Button>
          <Button
            variant="primary"
            disabled={!panels.data || !!panels.error}
            onClick={(event) => {
              trigger.current = event.currentTarget
              setEditor({ record: null })
            }}
          >
            <Plus aria-hidden /> 新建面板
          </Button>
        </div>
      </div>

      {panels.isLoading ? (
        <Skeleton className="h-20" />
      ) : panels.error ? (
        <ErrorNote
          error={panels.error}
          onRetry={() => {
            void panels.refetch()
          }}
        />
      ) : records.length === 0 ? (
        <p className="border-t border-paper-3 py-4 text-sm text-ink-2">这个场景还没有指令面板。</p>
      ) : (
        <ul className="divide-y divide-paper-3 border-t border-paper-3">
          {records.map((record) => (
            <li key={record.panel_id} className="flex flex-wrap items-center justify-between gap-x-5 gap-y-2 py-4">
              <div className="min-w-0 flex-1 basis-44">
                <h4 className="font-semibold wrap-anywhere text-ink">{record.panel?.remark || '指令面板'}</h4>
                <p className="mt-1 text-[13px] wrap-anywhere text-ink-3">
                  {record.target_type === 'specific' ? '指定会话' : '全部会话'}
                  {' · '}
                  {record.panel?.items?.length ?? 0} 项
                </p>
              </div>
              <div className="flex gap-2">
                <Button
                  size="sm"
                  onClick={(event) => {
                    trigger.current = event.currentTarget
                    setEditor({ record })
                  }}
                >
                  编辑
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={(event) => {
                    trigger.current = event.currentTarget
                    remove.reset()
                    setDeleting(record)
                  }}
                >
                  删除
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {editor && (
        <PanelEditor
          botId={bot.id}
          platformId={platform.id}
          scope={scope}
          record={editor.record}
          returnFocus={focusTrigger}
          onClose={() => setEditor(null)}
          onSaved={() => {
            setEditor(null)
            void panels.refetch()
          }}
        />
      )}
      <ConfirmDialog
        open={!!deleting}
        returnFocus={focusTrigger}
        onOpenChange={(open) => {
          if (!open && !remove.isPending) setDeleting(null)
        }}
        title="删除 QQ 指令面板？"
        body={
          <div className="grid gap-2">
            <p>删除后，该面板将从 QQ 中移除。</p>
            {remove.error && <ActionError error={remove.error} />}
          </div>
        }
        confirmLabel="确认删除"
        loading={remove.isPending}
        onConfirm={() => {
          if (deleting) remove.mutate(deleting)
        }}
      />
    </div>
  )
}

export default function QqPanels({ bot }: { bot: Bot }) {
  const accountId = useId()
  const [selected, setSelected] = useState('')
  const accounts = bot.platforms.filter((platform) => platform.kind === 'qq_official')
  const account = accounts.find((platform) => platform.id === selected) ?? accounts[0]
  return (
    <section className="grid gap-5 rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <div>
          <h2 className="font-display text-lg font-black text-ink">QQ 指令面板</h2>
          <p className="mt-1 text-sm leading-relaxed text-ink-2">点选指令会填入输入框，发送后才会触发。</p>
        </div>
        <a
          href={DOCS}
          target="_blank"
          rel="noreferrer"
          className="inline-flex min-h-9 items-center gap-1 text-sm font-semibold text-sky hover:underline"
        >
          QQ 文档 <ExternalLink className="size-3.5" aria-hidden />
        </a>
      </div>
      {accounts.length > 1 && (
        <Field label="QQ 机器人" htmlFor={accountId} className="max-w-sm">
          <Select id={accountId} value={account?.id} onChange={(event) => setSelected(event.target.value)}>
            {accounts.map((platform) => (
              <option key={platform.id} value={platform.id}>
                {platform.display_name} · {platform.app_id}
              </option>
            ))}
          </Select>
        </Field>
      )}
      {account ? (
        <PanelList key={account.id} bot={bot} platform={account} />
      ) : (
        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-paper-3 pt-4">
          <p className="text-sm text-ink-2">连接 QQ 官方机器人后即可管理指令。</p>
          <Link
            className="inline-flex min-h-10 items-center rounded-xl border border-paper-3 px-4 text-sm font-semibold text-ink hover:bg-white"
            to={`/app/bots/${bot.id}/connect`}
          >
            接入 QQ
          </Link>
        </div>
      )}
    </section>
  )
}
