/** Thin fetch wrapper with the error handling every provider adapter wants. */

export class ApiError extends Error {
  status: number
  url: string
  body?: string

  constructor(message: string, status: number, url: string, body?: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.url = url
    this.body = body
  }
}

export interface RequestOptions {
  method?: string
  headers?: Record<string, string>
  body?: unknown
  /** Send `body` as form-encoded rather than JSON (GitLab discussions want this). */
  form?: boolean
  /** Ask for text instead of parsing JSON. */
  raw?: boolean
  signal?: AbortSignal
  timeoutMs?: number
}

const DEFAULT_TIMEOUT = 30_000

function describe(status: number, url: string, body: string): string {
  const host = safeHost(url)
  if (status === 401) return `Not authorised on ${host} - the token is invalid or expired.`
  if (status === 403) {
    if (/rate limit/i.test(body)) return `Rate limited by ${host}. Try again shortly.`
    return `Forbidden on ${host} - the token is missing a required scope.`
  }
  if (status === 404) return `Not found on ${host} (${new URL(url).pathname}).`
  if (status >= 500) return `${host} returned a server error (${status}).`

  // Providers put the useful part in different fields; try the common ones.
  try {
    const parsed = JSON.parse(body) as Record<string, unknown>
    const detail =
      parsed.message ?? parsed.error ?? parsed.error_description ?? (parsed.errors as unknown)
    if (typeof detail === 'string') return detail
    if (Array.isArray(detail) && detail.length) return String(detail[0])
  } catch {
    /* body was not JSON */
  }
  return `${host} returned ${status}.`
}

function safeHost(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

export async function request<T>(url: string, options: RequestOptions = {}): Promise<T> {
  const { method = 'GET', headers = {}, body, form, raw, signal, timeoutMs = DEFAULT_TIMEOUT } = options

  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), timeoutMs)
  if (signal) signal.addEventListener('abort', () => controller.abort(), { once: true })

  const init: RequestInit = { method, signal: controller.signal, headers: { ...headers } }
  const sendHeaders = init.headers as Record<string, string>

  if (body !== undefined) {
    if (form) {
      sendHeaders['Content-Type'] = 'application/x-www-form-urlencoded'
      init.body = encodeForm(body as Record<string, unknown>)
    } else {
      sendHeaders['Content-Type'] = 'application/json'
      init.body = JSON.stringify(body)
    }
  }

  let response: Response
  try {
    response = await fetch(url, init)
  } catch (error) {
    clearTimeout(timer)
    if ((error as Error).name === 'AbortError') {
      throw new ApiError(`Request to ${safeHost(url)} timed out.`, 0, url)
    }
    throw new ApiError(`Could not reach ${safeHost(url)}: ${(error as Error).message}`, 0, url)
  }
  clearTimeout(timer)

  if (!response.ok) {
    const text = await response.text().catch(() => '')
    throw new ApiError(describe(response.status, url, text), response.status, url, text)
  }

  if (raw) return (await response.text()) as T
  if (response.status === 204) return undefined as T

  const text = await response.text()
  if (!text) return undefined as T
  try {
    return JSON.parse(text) as T
  } catch {
    return text as T
  }
}

/**
 * Flatten `{ position: { new_line: 3 } }` into `position[new_line]=3`, the bracket
 * notation Rails-based APIs (GitLab) expect from form posts.
 */
function encodeForm(source: Record<string, unknown>, prefix = ''): string {
  const parts: string[] = []
  for (const [key, value] of Object.entries(source)) {
    if (value === undefined || value === null) continue
    const name = prefix ? `${prefix}[${key}]` : key
    if (typeof value === 'object' && !Array.isArray(value)) {
      const nested = encodeForm(value as Record<string, unknown>, name)
      if (nested) parts.push(nested)
    } else if (Array.isArray(value)) {
      for (const entry of value) parts.push(`${encodeURIComponent(`${name}[]`)}=${encodeURIComponent(String(entry))}`)
    } else {
      parts.push(`${encodeURIComponent(name)}=${encodeURIComponent(String(value))}`)
    }
  }
  return parts.join('&')
}

/** Follow RFC 5988 `Link: <...>; rel="next"` pagination, capped so a huge account cannot hang a sync. */
export async function paginate<T>(
  url: string,
  options: RequestOptions,
  maxPages = 5,
): Promise<T[]> {
  const results: T[] = []
  let next: string | null = url

  for (let page = 0; page < maxPages && next; page++) {
    const response = await fetch(next, {
      method: 'GET',
      headers: options.headers,
      signal: options.signal,
    })
    if (!response.ok) {
      const text = await response.text().catch(() => '')
      throw new ApiError(describe(response.status, next, text), response.status, next, text)
    }
    const batch = (await response.json()) as T[]
    if (Array.isArray(batch)) results.push(...batch)
    next = parseNextLink(response.headers.get('link'))
  }
  return results
}

export function parseNextLink(header: string | null): string | null {
  if (!header) return null
  for (const part of header.split(',')) {
    const match = /<([^>]+)>\s*;\s*rel="?next"?/.exec(part.trim())
    if (match) return match[1]
  }
  return null
}

/** Normalise whatever the user typed into an origin: "gitlab.com" -> "https://gitlab.com". */
export function toOrigin(input: string): string {
  const trimmed = input.trim().replace(/\/+$/, '')
  if (!trimmed) throw new Error('A host is required.')
  const withScheme = /^https?:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`
  const url = new URL(withScheme)
  return `${url.protocol}//${url.host}${url.pathname === '/' ? '' : url.pathname.replace(/\/+$/, '')}`
}
