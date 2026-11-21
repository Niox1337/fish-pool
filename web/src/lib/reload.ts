const RELOAD_KEY = 'fish:chunk-reload-at'
const RELOAD_GUARD_MS = 30_000

/** True for errors raised when a lazily loaded page script cannot be fetched or parsed. */
export function isChunkError(error: unknown): boolean {
  const text = error instanceof Error ? `${error.name} ${error.message}` : String(error)
  return /dynamically imported module|Importing a module script failed|module script|ChunkLoadError|Unable to preload/i.test(text)
}

/**
 * Reloads the page so it picks up the current deployment.
 * Returns false without reloading when a reload already happened in the last 30 s or storage is unavailable.
 */
export function reloadForNewVersion(): boolean {
  try {
    const last = Number(sessionStorage.getItem(RELOAD_KEY) || 0)
    if (Date.now() - last < RELOAD_GUARD_MS) return false
    sessionStorage.setItem(RELOAD_KEY, String(Date.now()))
  } catch {
    return false
  }
  window.location.reload()
  return true
}
