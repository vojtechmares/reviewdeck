import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  agentCommand,
  agentCommandName,
  agentPrompt,
  fetchCommand,
  singleQuote,
} from '../src/shared/agent-prompt.ts'
import { DEFAULT_AGENT_COMMAND, DEFAULT_SETTINGS } from '../src/shared/types.ts'
import type { CommentThread, ProviderKind, ReviewItem } from '../src/shared/types.ts'

function item(provider: ProviderKind, overrides: Partial<ReviewItem> = {}): ReviewItem {
  return {
    id: 'acc:repo:88',
    accountId: 'acc',
    provider,
    repoKey: 'acme/design-tokens',
    repo: 'acme/design-tokens',
    number: 88,
    title: 'Regenerate the dark palette',
    url: 'https://example.test/pull/88',
    author: { name: 'lpeters', avatarUrl: '' },
    createdAt: '2026-08-01T10:00:00Z',
    updatedAt: '2026-08-01T10:00:00Z',
    draft: false,
    sourceBranch: 'design/contrast-pass',
    targetBranch: 'main',
    labels: [],
    myReviewState: 'pending',
    checks: { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] },
    ...overrides,
  }
}

function thread(overrides: Partial<CommentThread> = {}): CommentThread {
  return {
    id: 't1',
    comments: [
      {
        id: 'c1',
        author: { name: 'mnovotna', avatarUrl: '' },
        body: 'Can the timeout come from config?',
        createdAt: '2026-08-01T11:00:00Z',
      },
    ],
    resolved: false,
    outdated: false,
    canReply: false,
    canResolve: false,
    ...overrides,
  }
}

test('fetchCommand follows each host ref shape', () => {
  assert.equal(fetchCommand(item('github')), 'git fetch origin pull/88/head')
  assert.equal(fetchCommand(item('forgejo')), 'git fetch origin pull/88/head')
  assert.equal(fetchCommand(item('gitlab')), 'git fetch origin merge-requests/88/head')
})

test('fetchCommand falls back to the source branch where no ref shape exists', () => {
  // Bitbucket publishes no ref for a pull request, so the branch name is all there is.
  assert.equal(fetchCommand(item('bitbucket')), 'git fetch origin design/contrast-pass')
  assert.equal(
    fetchCommand(item('bitbucket', { sourceBranch: 'feature/x' })),
    'git fetch origin feature/x',
  )
})

test('agentCommandName prefers the account override', () => {
  assert.equal(
    agentCommandName({ agentCommand: 'claude-acme' }, { agentCommand: 'claude' }),
    'claude-acme',
  )
})

test('agentCommandName falls back to the setting, then to Claude Code', () => {
  assert.equal(agentCommandName(undefined, { agentCommand: 'my-claude' }), 'my-claude')
  assert.equal(agentCommandName({}, { agentCommand: 'my-claude' }), 'my-claude')

  // A blank override must not shadow the setting, and a blank setting must not
  // produce a command that is only a quoted prompt.
  assert.equal(
    agentCommandName({ agentCommand: '   ' }, { agentCommand: 'my-claude' }),
    'my-claude',
  )
  assert.equal(agentCommandName(undefined, { agentCommand: '' }), DEFAULT_AGENT_COMMAND)
  assert.equal(DEFAULT_SETTINGS.agentCommand, DEFAULT_AGENT_COMMAND)
})

test('the prompt carries the title, the base branch and the fetch command', () => {
  const prompt = agentPrompt(item('gitlab'), [])

  assert.match(prompt, /Regenerate the dark palette/)
  assert.match(prompt, /acme\/design-tokens #88/)
  assert.match(prompt, /git fetch origin merge-requests\/88\/head/)
  assert.match(prompt, /targets main/)
  assert.match(prompt, /git diff origin\/main\.\.\.FETCH_HEAD/)
  assert.match(prompt, /No open threads/)
})

test('the prompt lists open threads and leaves resolved ones out', () => {
  const prompt = agentPrompt(item('github'), [
    thread({ id: 'open', path: 'src/theme.ts', line: 42 }),
    thread({
      id: 'done',
      resolved: true,
      comments: [
        {
          id: 'c9',
          author: { name: 'hkramer', avatarUrl: '' },
          body: 'Already dealt with.',
          createdAt: '2026-08-01T12:00:00Z',
        },
      ],
    }),
  ])

  assert.match(prompt, /- src\/theme\.ts:42/)
  assert.match(prompt, /mnovotna: Can the timeout come from config\?/)
  assert.equal(prompt.includes('Already dealt with.'), false)
})

test('the prompt names an unanchored thread rather than pretending it has a line', () => {
  const prompt = agentPrompt(item('github'), [thread()])
  assert.match(prompt, /- On the pull request itself/)
})

test('the prompt flattens and caps a long comment so a bot report cannot swamp it', () => {
  const prompt = agentPrompt(item('github'), [
    thread({
      comments: [
        {
          id: 'c1',
          author: { name: 'dependabot', avatarUrl: '' },
          body: `line one\nline two${' padding'.repeat(200)}`,
          createdAt: '2026-08-01T11:00:00Z',
        },
      ],
    }),
  ])

  const line = prompt.split('\n').find((entry) => entry.includes('dependabot:'))
  assert.ok(line, 'the comment should appear')
  assert.ok(line.length < 300, `the comment line was ${line.length} characters`)
  assert.match(line, /dependabot: line one line two/)
})

test('the prompt asks for findings one path-and-line-prefixed line at a time', () => {
  const prompt = agentPrompt(item('github'), [])
  assert.match(prompt, /exactly one line/)
  assert.match(prompt, /path\/to\/file\.ext:LINE:/)
  assert.match(prompt, /output nothing else/)
  assert.match(prompt, /no findings/)
})

test('singleQuote survives an apostrophe in the prompt', () => {
  assert.equal(singleQuote('plain'), "'plain'")
  assert.equal(singleQuote("don't"), "'don'\\''t'")
})

test('agentCommand is the resolved name applied to the quoted prompt', () => {
  const command = agentCommand(
    item('github', { title: "Don't drop the retry" }),
    [],
    { agentCommand: 'claude-acme' },
    { agentCommand: 'claude' },
  )

  assert.ok(command.startsWith("claude-acme '"), command.slice(0, 40))
  assert.ok(command.endsWith("'"))
  // The apostrophe in the title must not close the quoting early.
  assert.match(command, /Don'\\''t drop the retry/)
})
