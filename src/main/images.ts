/**
 * Serves the custom scheme that carries an account's images.
 *
 * The token stays here. The renderer only ever asks for a URL under this scheme;
 * what it is allowed to ask for, and which account's credential goes with it, is
 * decided by `resolveImageRequest` in the shared layer, which is where the check
 * that a token cannot reach a host it does not belong to lives.
 */

import { protocol } from 'electron'
import { accountHosts, IMAGE_SCHEME, resolveImageRequest } from '@shared/images.ts'
import type { Account, ProviderKind } from '@shared/types.ts'
import { getAccount, getToken, listAccounts } from './store.ts'

const FETCH_TIMEOUT = 20_000

/** Enough for the usual hop to object storage, not enough to be walked in circles. */
const MAX_REDIRECTS = 4

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
      const upstream = await fetchImage(account, token, resolved.target)
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

/**
 * Follows redirects by hand, so the credential stops at the account's own hosts.
 *
 * `redirect: 'follow'` would not do: fetch strips `Authorization` when a redirect
 * crosses origins, but GitLab's `PRIVATE-TOKEN` is an ordinary header and would be
 * carried straight on. An upload URL that hops to object storage is the everyday
 * case, and an open redirect on the instance is the adversarial one - in both the
 * token has no business at the far end.
 *
 * Once dropped the credential never comes back, so a chain that returns to the
 * account's host cannot pick it up again.
 */
async function fetchImage(account: Account, token: string, target: string): Promise<Response> {
  const signal = AbortSignal.timeout(FETCH_TIMEOUT)
  const hosts = accountHosts(account)
  let current = target
  let credentialed = true

  for (let hop = 0; ; hop++) {
    const response = await fetch(current, {
      headers: {
        ...(credentialed ? authorization(account, token) : { 'User-Agent': 'Reviewdeck' }),
        Accept: 'image/*,*/*;q=0.8',
      },
      redirect: 'manual',
      signal,
    })

    const location = response.status >= 300 && response.status < 400 && response.headers.get('location')
    if (!location) return response
    if (hop >= MAX_REDIRECTS) return new Response(null, { status: 508 })

    let next: URL
    try {
      next = new URL(location, current)
    } catch {
      return new Response(null, { status: 502 })
    }
    if (next.protocol !== 'https:' && next.protocol !== 'http:') {
      return new Response(null, { status: 502 })
    }

    credentialed = credentialed && hosts.includes(next.host.toLowerCase())
    current = next.toString()
  }
}
