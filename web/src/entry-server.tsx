import { prerender } from 'react-dom/static'
import { StaticRouter } from 'react-router'
import { createQueryClient } from './queryClient'
import { Root } from './Root'

export { headTags, PRERENDERED, SITE_URL } from './seo'

/** HTML of the app at `path`, after every lazy route has loaded. */
export async function render(path: string): Promise<string> {
  const app = <Root client={createQueryClient()} router={(children) => <StaticRouter location={path}>{children}</StaticRouter>} />
  const { prelude } = await prerender(app)
  return new Response(prelude).text()
}
