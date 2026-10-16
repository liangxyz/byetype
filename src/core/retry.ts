/**
 * Retry an async loader a bounded number of times.
 *
 * Used for the settings page's initial config load: the settings webview is
 * created at app start and can invoke backend commands before the backend has
 * finished registering its managed state. That first call then rejects, and
 * without a retry the page would stay on "Loading..." until manually refreshed.
 */
export async function loadWithRetry<T>(
  load: () => Promise<T>,
  options: {
    attempts: number
    delayMs: number
    isCancelled?: () => boolean
    sleep?: (ms: number) => Promise<void>
    onRetry?: (error: unknown, attempt: number) => void
  },
): Promise<T> {
  const sleep = options.sleep ?? (ms => new Promise<void>(resolve => setTimeout(resolve, ms)))
  let lastError: unknown
  for (let attempt = 1; attempt <= options.attempts; attempt++) {
    if (options.isCancelled?.()) throw new Error('cancelled')
    try {
      return await load()
    } catch (error) {
      lastError = error
      if (attempt === options.attempts) break
      options.onRetry?.(error, attempt)
      await sleep(options.delayMs)
    }
  }
  throw lastError
}

export function errorToMessage(error: unknown, fallback: string): string {
  if (typeof error === 'string' && error) return error
  const message = (error as { message?: unknown } | null)?.message
  return typeof message === 'string' && message ? message : fallback
}
