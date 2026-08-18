import { test } from 'node:test'
import assert from 'node:assert/strict'
import { limitConcurrency, rollUp, summariseChecks } from '../src/main/providers/types.ts'
import { graphqlRoot } from '../src/main/providers/github.ts'
import type { CheckStatus } from '../src/shared/types.ts'

test('rollUp lets the worst status win', () => {
  assert.equal(rollUp([]), 'unknown')
  assert.equal(rollUp(['passed', 'passed']), 'passed')
  assert.equal(rollUp(['passed', 'running']), 'running')
  // A failure outranks anything still in flight.
  assert.equal(rollUp(['running', 'failed']), 'failed')
  assert.equal(rollUp(['unknown', 'unknown']), 'unknown')
  // A single real pass alongside unknowns still counts as passing.
  assert.equal(rollUp(['unknown', 'passed']), 'passed')
})

test('summariseChecks counts each bucket', () => {
  const runs: { status: CheckStatus }[] = [
    { status: 'passed' },
    { status: 'passed' },
    { status: 'failed' },
    { status: 'running' },
    { status: 'unknown' },
  ]
  assert.deepEqual(summariseChecks(runs), {
    status: 'failed',
    passed: 2,
    failed: 1,
    running: 1,
    total: 5,
  })
})

test('limitConcurrency preserves order and caps parallelism', async () => {
  let active = 0
  let peak = 0
  const input = Array.from({ length: 12 }, (_, i) => i)

  const result = await limitConcurrency(input, 3, async (value) => {
    active++
    peak = Math.max(peak, active)
    await new Promise((resolve) => setTimeout(resolve, 5))
    active--
    return value * 2
  })

  assert.deepEqual(result, input.map((value) => value * 2))
  assert.ok(peak <= 3, `peak concurrency was ${peak}`)
})

test('limitConcurrency handles an empty list', async () => {
  assert.deepEqual(await limitConcurrency([], 4, async () => 1), [])
})

test('graphqlRoot follows GitHub.com and Enterprise Server apart', () => {
  // GitHub.com serves GraphQL on the API host it already uses.
  assert.equal(graphqlRoot('https://api.github.com'), 'https://api.github.com/graphql')

  // Enterprise Server serves it beside the REST root, one segment up from /api/v3.
  assert.equal(graphqlRoot('https://github.acme.com/api/v3'), 'https://github.acme.com/api/graphql')
  assert.equal(graphqlRoot('https://github.acme.com/api/v3/'), 'https://github.acme.com/api/graphql')
  assert.equal(
    graphqlRoot('https://acme.dev/github/api/v3'),
    'https://acme.dev/github/api/graphql',
  )
})
