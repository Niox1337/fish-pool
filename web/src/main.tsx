import { createRoot, hydrateRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router'
import { reloadForNewVersion } from './components/ErrorBoundary'
import { createQueryClient } from './queryClient'
import { Root } from './Root'
import './index.css'

window.addEventListener('vite:preloadError', (e) => {
  if (reloadForNewVersion()) e.preventDefault()
})

const container = document.getElementById('root')!
const app = <Root client={createQueryClient()} router={(children) => <BrowserRouter>{children}</BrowserRouter>} />

// The static HTML holds the page rendered for `data-route`. An inline script in index.html empties it on other paths.
if (container.dataset.route === window.location.pathname && container.firstChild) {
  hydrateRoot(container, app)
} else {
  createRoot(container).render(app)
}
