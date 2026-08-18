import { test } from 'node:test'
import assert from 'node:assert/strict'
import { tokenCreateUrl } from '../src/shared/token-url.ts'

test('tokenCreateUrl follows the typed Forgejo host', () => {
  assert.equal(
    tokenCreateUrl('forgejo', 'codeberg.org'),
    'https://codeberg.org/user/settings/applications',
  )
  assert.equal(
    tokenCreateUrl('forgejo', 'git.acme.dev'),
    'https://git.acme.dev/user/settings/applications',
  )
  assert.equal(
    tokenCreateUrl('forgejo', 'https://git.acme.dev/'),
    'https://git.acme.dev/user/settings/applications',
  )
  assert.equal(
    tokenCreateUrl('forgejo', 'http://localhost:3000'),
    'http://localhost:3000/user/settings/applications',
  )
  assert.equal(
    tokenCreateUrl('forgejo', 'https://acme.dev/git/'),
    'https://acme.dev/git/user/settings/applications',
  )
})

test('tokenCreateUrl falls back when the host is empty or invalid', () => {
  assert.equal(
    tokenCreateUrl('forgejo', '   '),
    'https://codeberg.org/user/settings/applications',
  )
  assert.equal(
    tokenCreateUrl('forgejo', '://'),
    'https://codeberg.org/user/settings/applications',
  )
})

test('tokenCreateUrl follows custom GitLab and GHES hosts', () => {
  assert.equal(
    tokenCreateUrl('gitlab', 'gitlab.acme.com'),
    'https://gitlab.acme.com/-/user_settings/personal_access_tokens',
  )
  assert.equal(
    tokenCreateUrl('github', 'github.acme.com'),
    'https://github.acme.com/settings/tokens/new?scopes=repo,read:org&description=Reviewdeck',
  )
  assert.equal(
    tokenCreateUrl('github', 'api.github.com'),
    'https://github.com/settings/tokens/new?scopes=repo,read:org&description=Reviewdeck',
  )
})

test('tokenCreateUrl keeps Bitbucket on bitbucket.org', () => {
  assert.equal(
    tokenCreateUrl('bitbucket', 'ignored.example'),
    'https://bitbucket.org/account/settings/app-passwords/new',
  )
})
