// Renders the pages listed in src/seo.ts to static HTML after the client build and writes sitemap.xml.
import { readFile, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'

const web = resolve(import.meta.dirname, '..')
const dist = join(web, 'dist')
// Inside node_modules so the server bundle resolves the app's dependencies.
const out = join(web, 'node_modules', '.prerender')

try {
  await build({
    root: web,
    logLevel: 'warn',
    build: { ssr: 'src/entry-server.tsx', outDir: out, emptyOutDir: true, copyPublicDir: false },
  })
  const { render, headTags, PRERENDERED, SITE_URL } = await import(pathToFileURL(join(out, 'entry-server.js')).href)
  const template = await readFile(join(dist, 'index.html'), 'utf8')
  const seo = /<!--seo:start-->[\s\S]*?<!--seo:end-->/
  if (!seo.test(template) || !template.includes('<div id="root"></div>')) {
    throw new Error('dist/index.html lacks the seo markers or the empty root element')
  }
  for (const path of Object.keys(PRERENDERED)) {
    const body = await render(path)
    const html = template
      .replace(seo, headTags(path))
      .replace('<div id="root"></div>', () => `<div id="root" data-route="${path}">${body}</div>`)
    const file = path === '/' ? 'index.html' : `${path.slice(1)}.html`
    await writeFile(join(dist, file), html)
    console.log(`prerendered ${path} -> dist/${file} (${body.length} bytes)`)
  }
  const today = new Date().toISOString().slice(0, 10)
  const urls = Object.keys(PRERENDERED)
    .map((path) => `  <url>\n    <loc>${SITE_URL}${path}</loc>\n    <lastmod>${today}</lastmod>\n  </url>`)
    .join('\n')
  await writeFile(
    join(dist, 'sitemap.xml'),
    `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls}\n</urlset>\n`,
  )
} finally {
  await rm(out, { recursive: true, force: true })
}
