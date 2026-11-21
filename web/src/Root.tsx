import { StrictMode, type ReactNode } from 'react'
import { QueryClientProvider, type QueryClient } from '@tanstack/react-query'
import { Toaster } from 'sonner'
import App from './App'

/** The app tree shared by the browser entry and the build time prerender. `router` wraps the app. */
export function Root({ client, router }: { client: QueryClient; router: (children: ReactNode) => ReactNode }) {
  return (
    <StrictMode>
      <QueryClientProvider client={client}>
        {router(
          <>
            <App />
            <Toaster
              position="top-center"
              toastOptions={{
                className: '!rounded-2xl !border-paper-3 !bg-cream !text-ink !font-sans !shadow-[var(--shadow-lift)]',
              }}
            />
          </>,
        )}
      </QueryClientProvider>
    </StrictMode>
  )
}
