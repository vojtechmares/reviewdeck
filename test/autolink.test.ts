import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  autolinkSpans,
  emojiFor,
  issueUrl,
  mentionUrl,
  renderEmoji,
  repositoryRoot,
} from '../src/shared/autolink.ts'

test('repositoryRoot strips the pull request segment off each host URL shape', () => {
  assert.equal(
    repositoryRoot({ provider: 'github', url: 'https://github.com/acme/checkout-api/pull/412' }),
    'https://github.com/acme/checkout-api',
  )
  assert.equal(
    repositoryRoot({
      provider: 'gitlab',
      url: 'https://gitlab.acme.dev/group/sub/project/-/merge_requests/88',
    }),
    'https://gitlab.acme.dev/group/sub/project',
  )
  assert.equal(
    repositoryRoot({ provider: 'forgejo', url: 'https://codeberg.org/vmares/dotfiles/pulls/9' }),
    'https://codeberg.org/vmares/dotfiles',
  )
  assert.equal(
    repositoryRoot({
      provider: 'bitbucket',
      url: 'https://bitbucket.org/acme/website/pull-requests/12',
    }),
    'https://bitbucket.org/acme/website',
  )
})

test('repositoryRoot recovers the GitLab root the naive construction would get wrong', () => {
  // The GitLab adapter keeps the numeric project id in repoKey, so joining the
  // account web root to it would build https://gitlab.acme.dev/4711 - a dead link.
  // The item's own URL is the only place the real path survives.
  const item = {
    provider: 'gitlab' as const,
    url: 'https://gitlab.acme.dev/acme/design-tokens/-/merge_requests/88',
  }
  assert.equal(repositoryRoot(item), 'https://gitlab.acme.dev/acme/design-tokens')

  // Older GitLab served merge requests without the /-/ separator.
  assert.equal(
    repositoryRoot({ provider: 'gitlab', url: 'https://gitlab.acme.dev/acme/x/merge_requests/3' }),
    'https://gitlab.acme.dev/acme/x',
  )
})

test('repositoryRoot gives up rather than guessing at an unexpected URL', () => {
  assert.equal(repositoryRoot({ provider: 'github', url: 'https://github.com/acme/repo' }), null)
  assert.equal(repositoryRoot({ provider: 'github', url: '' }), null)
})

test('mentionUrl points at the profile on the host web root', () => {
  assert.equal(mentionUrl('github', 'https://github.com', 'octocat'), 'https://github.com/octocat')
  assert.equal(
    mentionUrl('gitlab', 'https://gitlab.acme.dev/', 'vmares'),
    'https://gitlab.acme.dev/vmares',
  )
  assert.equal(
    mentionUrl('forgejo', 'https://codeberg.org', 'vmares'),
    'https://codeberg.org/vmares',
  )
})

test('mentionUrl produces nothing where the host has no profile path', () => {
  // A Bitbucket mention addresses an account id, not the name in the text.
  assert.equal(mentionUrl('bitbucket', 'https://bitbucket.org', 'someone'), null)
  assert.equal(mentionUrl('github', '', 'octocat'), null)
})

test('issueUrl uses the right path shape per provider', () => {
  assert.equal(
    issueUrl('github', 'https://github.com/acme/api', 12),
    'https://github.com/acme/api/issues/12',
  )
  assert.equal(
    issueUrl('gitlab', 'https://gitlab.acme.dev/acme/api', 12),
    'https://gitlab.acme.dev/acme/api/-/issues/12',
  )
  assert.equal(
    issueUrl('forgejo', 'https://codeberg.org/vmares/dotfiles', 12),
    'https://codeberg.org/vmares/dotfiles/issues/12',
  )
})

test('issueUrl produces nothing without a host path or a repository root', () => {
  assert.equal(issueUrl('bitbucket', 'https://bitbucket.org/acme/web', 12), null)
  assert.equal(issueUrl('github', null, 12), null)
})

test('autolinkSpans finds mentions and references in ordinary text', () => {
  assert.deepEqual(autolinkSpans('cc @mnovotna about #412 please'), [
    { kind: 'mention', start: 3, end: 12, text: '@mnovotna', handle: 'mnovotna' },
    { kind: 'issue', start: 19, end: 23, text: '#412', number: 412 },
  ])

  const [start] = autolinkSpans('@vmares opened it')
  assert.deepEqual(start, { kind: 'mention', start: 0, end: 7, text: '@vmares', handle: 'vmares' })
})

test('autolinkSpans needs a boundary, so an address or a colour is not a reference', () => {
  assert.deepEqual(autolinkSpans('write to vojtech@mares.cz'), [])
  assert.deepEqual(autolinkSpans('the colour is #ff8800 now'), [])
  assert.deepEqual(autolinkSpans('see https://example.test/@someone'), [])
})

test('autolinkSpans leaves trailing punctuation out of a handle', () => {
  assert.deepEqual(autolinkSpans('thanks @lpeters.'), [
    { kind: 'mention', start: 7, end: 15, text: '@lpeters', handle: 'lpeters' },
  ])
})

test('emojiFor resolves a known shortcode and refuses an unknown one', () => {
  assert.equal(emojiFor('tada'), '🎉')
  assert.equal(emojiFor('white_check_mark'), '✅')
  assert.equal(emojiFor('+1'), '👍')
  assert.equal(emojiFor('not_an_emoji_we_carry'), null)
})

test('renderEmoji replaces what it knows and leaves the rest as typed', () => {
  assert.equal(renderEmoji('Shipped :tada: at last'), 'Shipped 🎉 at last')
  assert.equal(renderEmoji('nothing :obscure_thing: here'), 'nothing :obscure_thing: here')
  assert.equal(renderEmoji('ratio 3:4:5 unchanged'), 'ratio 3:4:5 unchanged')
})
