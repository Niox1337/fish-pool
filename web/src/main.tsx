import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter } from 'react-router'
import { Toaster } from 'sonner'
import App from './App'
import { reloadForNewVersion } from './components/ErrorBoundary'
import './index.css'

window.addEventListener('vite:preloadError', (e) => {
  if (reloadForNewVersion()) e.preventDefault()
})

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: (n, e) => n < 2 && !(e as { status?: number }).status, refetchOnWindowFocus: true },
  },
})

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <App />
        <Toaster
          position="top-center"
          toastOptions={{
            className: '!rounded-2xl !border-paper-3 !bg-cream !text-ink !font-sans !shadow-[var(--shadow-lift)]',
          }}
        />
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
)
