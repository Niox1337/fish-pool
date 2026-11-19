import { toast } from 'sonner'

export async function copyText(text: string, what = '内容') {
  try {
    await navigator.clipboard.writeText(text)
    toast.success(`${what}已复制`)
  } catch {
    toast.error('浏览器没让复制，请手动选中后复制')
  }
}
