import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  accountHosts,
  IMAGE_SCHEME,
  proxyImageUrl,
  resolveImageRequest,
  rewriteImageSource,
  type ImageAccount,
} from '../src/shared/images.ts'

const GITLAB: ImageAccount = {
  id: 'acct-gitlab',
  baseUrl: 'https://gitlab.acme.dev/api/v4',
  webUrl: 'https://gitlab.acme.dev',
}

const FORGEJO: ImageAccount = {
  id: 'acct-forgejo',
  baseUrl: 'https://codeberg.org/api/v1',
  webUrl: 'https://codeberg.org',
}

const ACCOUNTS = [GITLAB, FORGEJO]

test('accountHosts covers the web root and the API root without repeating one', () => {
  assert.deepEqual(accountHosts(GITLAB), ['gitlab.acme.dev'])
  assert.deepEqual(
    accountHosts({ id: 'x', webUrl: 'https://github.com', baseUrl: 'https://api.github.com' }),
    ['github.com', 'api.github.com'],
  )
  assert.deepEqual(accountHosts({ id: 'x', webUrl: 'not a url', baseUrl: '' }), [])
})

test('rewriteImageSource redirects a source on a signed-in host', () => {
  const source = 'https://gitlab.acme.dev/acme/api/uploads/abc123/shot.png'
  const rewritten = rewriteImageSource(source, { repoRoot: null, accounts: ACCOUNTS })

  assert.notEqual(rewritten, source)
  assert.deepEqual(resolveImageRequest(rewritten, ACCOUNTS), {
    accountId: 'acct-gitlab',
    target: source,
  })
})

test('rewriteImageSource leaves a source on any other host alone', () => {
  // A public badge or an external screenshot keeps loading over ordinary HTTPS,
  // with no credential anywhere near it.
  for (const source of [
    'https://img.shields.io/badge/build-passing-green.svg',
    'https://user-images.githubusercontent.com/1/shot.png',
    'https://gitlab.example.com/other/uploads/x.png',
    'data:image/png;base64,iVBORw0KGgo=',
  ]) {
    assert.equal(rewriteImageSource(source, { repoRoot: null, accounts: ACCOUNTS }), source)
  }
})

test('rewriteImageSource resolves a relative source against the repository root first', () => {
  const context = { repoRoot: 'https://codeberg.org/vmares/dotfiles', accounts: ACCOUNTS }

  // Whether it needs a credential depends on the host it lands on, so it has to be
  // made absolute before the decision, not after.
  const rewritten = rewriteImageSource('docs/shot.png', context)
  assert.deepEqual(resolveImageRequest(rewritten, ACCOUNTS), {
    accountId: 'acct-forgejo',
    target: 'https://codeberg.org/vmares/dotfiles/docs/shot.png',
  })

  const rooted = rewriteImageSource('/attachments/abc', context)
  assert.deepEqual(resolveImageRequest(rooted, ACCOUNTS), {
    accountId: 'acct-forgejo',
    target: 'https://codeberg.org/attachments/abc',
  })
})

test('rewriteImageSource leaves a relative source alone with no root to resolve it against', () => {
  assert.equal(
    rewriteImageSource('docs/shot.png', { repoRoot: null, accounts: ACCOUNTS }),
    'docs/shot.png',
  )
  assert.equal(rewriteImageSource('   ', { repoRoot: null, accounts: ACCOUNTS }), '   ')
})

test('resolveImageRequest rejects a host that is not a signed-in account', () => {
  // The whole security boundary: the handler attaches a credential to whatever this
  // accepts, so a body written by someone else must not be able to name a host of
  // its choosing and have a real account's token posted to it.
  const forged = `${IMAGE_SCHEME}://fetch/?account=acct-gitlab&url=${encodeURIComponent('https://evil.test/steal')}`
  assert.equal(resolveImageRequest(forged, ACCOUNTS), null)

  // Nor by pairing one account with another account's host.
  const crossed = proxyImageUrl(GITLAB.id, 'https://codeberg.org/attachments/abc')
  assert.equal(resolveImageRequest(crossed, ACCOUNTS), null)

  // Nor a host that merely looks like one.
  const lookalike = proxyImageUrl(GITLAB.id, 'https://gitlab.acme.dev.evil.test/x.png')
  assert.equal(resolveImageRequest(lookalike, ACCOUNTS), null)
})

test('resolveImageRequest rejects an unknown account', () => {
  const unknown = proxyImageUrl('acct-gone', 'https://gitlab.acme.dev/uploads/x.png')
  assert.equal(resolveImageRequest(unknown, ACCOUNTS), null)
  assert.equal(resolveImageRequest(unknown, []), null)
})

test('resolveImageRequest rejects a malformed request rather than throwing', () => {
  for (const request of [
    '',
    'not a url at all',
    'https://gitlab.acme.dev/uploads/x.png',
    `${IMAGE_SCHEME}://fetch/`,
    `${IMAGE_SCHEME}://fetch/?account=acct-gitlab`,
    `${IMAGE_SCHEME}://fetch/?url=${encodeURIComponent('https://gitlab.acme.dev/x.png')}`,
    `${IMAGE_SCHEME}://fetch/?account=acct-gitlab&url=%%%`,
    proxyImageUrl(GITLAB.id, 'file:///etc/passwd'),
    proxyImageUrl(GITLAB.id, `${IMAGE_SCHEME}://fetch/?account=acct-gitlab&url=x`),
  ]) {
    assert.equal(resolveImageRequest(request, ACCOUNTS), null, `accepted: ${request}`)
  }
})

test('resolveImageRequest accepts a request it can vouch for, whatever the case of the host', () => {
  const request = proxyImageUrl(GITLAB.id, 'https://GitLab.Acme.Dev/acme/api/uploads/a/shot.png')

  assert.deepEqual(resolveImageRequest(request, ACCOUNTS), {
    accountId: 'acct-gitlab',
    target: 'https://gitlab.acme.dev/acme/api/uploads/a/shot.png',
  })
})
