/**
 * Serves the custom scheme that carries an account's images.
 *
 * The token stays here. The renderer only ever asks for a URL under this scheme;
 * what it is allowed to ask for, and which account's credential goes with it, is
 * decided by `resolveImageRequest` in the shared layer, which is where the check
 * that a token cannot reach a host it does not belong to lives.
 */

import { protocol } from 'electron'
import { IMAGE_SCHEME, resolveImageRequest } from '@shared/images.ts'
import type { Account, ProviderKind } from '@shared/types.ts'
import { getAccount, getToken, listAccounts } from './store.ts'

const FETCH_TIMEOUT = 20_000

/**
 * How each host expects a token. The same shapes the adapters send - an upload URL
 * is served by the instance itself, not by the API, but it accepts the same
 * credential.
 */
function authorization(account: Account, token: string): Record<string, string> {
  const byKind: Record<ProviderKind, Record<string, string>> = {
    github: { Authorization: `Bearer ${token}` },
    gitlab: { 'PRIVATE-TOKEN': token },
    forgejo: { Authorization: `token ${token}` },
    bitbucket: {
      Authorization: `Basic ${Buffer.from(`${account.username}:${token}`).toString('base64')}`,
    },
  }
  return { ...byKind[account.kind], 'User-Agent': 'Reviewdeck' }
}

/**
 * Must run before the app is ready. `standard` gives the scheme a host component so
 * it parses predictably, and `stream` lets a response body be piped through rather
 * than buffered.
 */
export function registerImageScheme(): void {
  protocol.registerSchemesAsPrivileged([
    {
      scheme: IMAGE_SCHEME,
      privileges: { standard: true, secure: true, supportFetchAPI: true, stream: true },
    },
  ])
}

export function serveImages(): void {
  protocol.handle(IMAGE_SCHEME, async (request) => {
    const resolved = resolveImageRequest(request.url, listAccounts())
    // Anything we cannot vouch for fails the image, which shows its alt text.
    if (!resolved) return new Response(null, { status: 400 })

    const account = getAccount(resolved.accountId)
    if (!account) return new Response(null, { status: 404 })

    let token: string
    try {
      token = getToken(account.id)
    } catch {
      return new Response(null, { status: 401 })
    }

    try {
      const upstream = await fetch(resolved.target, {
        headers: { ...authorization(account, token), Accept: 'image/*,*/*;q=0.8' },
        signal: AbortSignal.timeout(FETCH_TIMEOUT),
        redirect: 'follow',
      })
      if (!upstream.ok) return new Response(null, { status: upstream.status })

      const type = upstream.headers.get('content-type')
      return new Response(upstream.body, {
        status: 200,
        headers: type ? { 'Content-Type': type } : {},
      })
    } catch {
      // A host that is down or slow is a broken image, not a broken app.
      return new Response(null, { status: 502 })
    }
  })
}
