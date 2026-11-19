import { useNavigate } from 'react-router'
import { toast } from 'sonner'
import { ApiError } from '@/api/client'

export function errMsg(e: unknown, fallback = '出了点问题，稍后再试') {
  return e instanceof ApiError || e instanceof Error ? e.message : fallback
}

export const isExpired = (e: unknown) => e instanceof ApiError && e.code === 'service_expired'

export function useErrorToast() {
  const navigate = useNavigate()
  return (e: unknown, fallback?: string) => {
    if (isExpired(e)) {
      toast.error(errMsg(e), { action: { label: '去兑换', onClick: () => navigate('/app/account#redeem') } })
    } else {
      toast.error(errMsg(e, fallback))
    }
  }
}
