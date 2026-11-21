import { QueryClient } from '@tanstack/react-query'

export function createQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: (n, e) => n < 2 && !(e as { status?: number }).status, refetchOnWindowFocus: true },
    },
  })
}
