import type { ProviderKind } from './types.ts'

const GITHUB_COM = new Set(['github.com', 'www.github.com', 'api.github.com'])

const PATHS: Record<ProviderKind, string> = {
  github: '/settings/tokens/new?scopes=repo,read:org&description=Reviewdeck',
  gitlab: '/-/user_settings/personal_access_tokens',
  forgejo: '/user/settings/applications',
  bitbucket: '/account/settings/app-passwords/new',
}

const FALLBACKS: Record<ProviderKind, string> = {
  github: `https://github.com${PATHS.github}`,
  gitlab: `https://gitlab.com${PATHS.gitlab}`,
  forgejo: `https://codeberg.org${PATHS.forgejo}`,
  bitbucket: `https://bitbucket.org${PATHS.bitbucket}`,
}

export function tokenCreateUrl(kind: ProviderKind, host: string): string {
  if (kind === 'bitbucket') return FALLBACKS.bitbucket
  const origin = tryOrigin(host)
  if (!origin) return FALLBACKS[kind]
  if (kind === 'github') {
    const hostname = new URL(origin).hostname
    const web = GITHUB_COM.has(hostname) ? 'https://github.com' : origin
    return `${web}${PATHS.github}`
  }
  return `${origin}${PATHS[kind]}`
}

function tryOrigin(input: string): string | null {
  const trimmed = input.trim().replace(/\/+$/, '')
  if (!trimmed) return null
  try {
    const withScheme = /^https?:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`
    const url = new URL(withScheme)
    return `${url.protocol}//${url.host}${url.pathname === '/' ? '' : url.pathname.replace(/\/+$/, '')}`
  } catch {
    return null
  }
}
