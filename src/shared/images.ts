/**
 * Images that sit behind an account's authentication.
 *
 * A screenshot pasted into a GitLab merge request becomes an upload on that
 * instance, and a Forgejo attachment behaves the same way. On a private project
 * both need a token, and an image tag cannot carry an authorization header - so on
 * exactly the self-hosted hosts this app exists for, description screenshots come
 * out broken.
 *
 * The way round it is a custom scheme the main process serves: the renderer points
 * an image at it, and the main process does the authenticated fetch. Both halves of
 * that agreement live here, so the URL the renderer builds and the request the
 * handler takes apart can never drift.
 *
 * Everything in this module is pure and electron-free, which matters most for
 * `resolveImageRequest`: it is the security boundary of the feature, because the
 * handler attaches a credential to whatever it accepts.
 */

export const IMAGE_SCHEME = 'reviewdeck-image'

/** Registered as a standard scheme, so it parses with a host component. */
const PROXY_ORIGIN = `${IMAGE_SCHEME}://fetch`

export interface ImageAccount {
  id: string
  /** API root, e.g. https://gitlab.acme.dev/api/v4 */
  baseUrl: string
  /** Web root, e.g. https://gitlab.acme.dev */
  webUrl: string
}

function hostOf(url: string): string | null {
  try {
    return new URL(url).host.toLowerCase()
  } catch {
    return null
  }
}

/** The hosts an account's token belongs to, and the only ones it may be sent to. */
export function accountHosts(account: ImageAccount): string[] {
  const hosts = [hostOf(account.webUrl), hostOf(account.baseUrl)].filter(
    (host): host is string => host !== null,
  )
  return [...new Set(hosts)]
}

export function accountForHost(
  accounts: ImageAccount[],
  host: string | null,
): ImageAccount | undefined {
  if (!host) return undefined
  const wanted = host.toLowerCase()
  return accounts.find((account) => accountHosts(account).includes(wanted))
}

/** The URL an image on a signed-in account's host is pointed at instead. */
export function proxyImageUrl(accountId: string, target: string): string {
  const url = new URL(PROXY_ORIGIN)
  url.searchParams.set('account', accountId)
  url.searchParams.set('url', target)
  return url.toString()
}

export interface ImageContext {
  /** Repository web root, so a relative source can be made absolute first. */
  repoRoot: string | null
  accounts: ImageAccount[]
}

/**
 * The rewrite decision for one image source.
 *
 * A source on a signed-in account's host is pointed at the proxy; everything else
 * is returned exactly as it came in, so a public badge or an external screenshot
 * keeps loading over ordinary HTTPS with no credential involved.
 *
 * A relative source is resolved against the repository root first, because whether
 * it needs authenticating depends on the host it lands on.
 */
export function rewriteImageSource(source: string, context: ImageContext): string {
  const absolute = absoluteSource(source, context.repoRoot)
  if (!absolute) return source

  const account = accountForHost(context.accounts, hostOf(absolute))
  return account ? proxyImageUrl(account.id, absolute) : source
}

function absoluteSource(source: string, repoRoot: string | null): string | null {
  const trimmed = source.trim()
  if (!trimmed || trimmed.startsWith('data:')) return null
  try {
    // A relative source only means something against the repository it was written in.
    return new URL(trimmed, repoRoot ? `${repoRoot.replace(/\/+$/, '')}/` : undefined).toString()
  } catch {
    return null
  }
}

export interface ResolvedImageRequest {
  accountId: string
  target: string
}

/**
 * Takes a proxy request back apart, and refuses anything it cannot vouch for.
 *
 * The host of the target has to belong to the account the request names. Checking
 * the account exists would not be enough on its own: a body written by someone else
 * could otherwise pair a real account with a host of their choosing and have the
 * handler post that account's token to it.
 *
 * Returns null rather than throwing, so a malformed request is a failed image and
 * not a crashed handler.
 */
export function resolveImageRequest(
  requestUrl: string,
  accounts: ImageAccount[],
): ResolvedImageRequest | null {
  let parsed: URL
  try {
    parsed = new URL(requestUrl)
  } catch {
    return null
  }
  if (parsed.protocol !== `${IMAGE_SCHEME}:`) return null

  const accountId = parsed.searchParams.get('account')
  const target = parsed.searchParams.get('url')
  if (!accountId || !target) return null

  let destination: URL
  try {
    destination = new URL(target)
  } catch {
    return null
  }
  if (destination.protocol !== 'https:' && destination.protocol !== 'http:') return null

  const account = accounts.find((candidate) => candidate.id === accountId)
  if (!account) return null
  if (!accountHosts(account).includes(destination.host.toLowerCase())) return null

  return { accountId: account.id, target: destination.toString() }
}
