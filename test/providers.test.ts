import { test } from 'node:test'
import assert from 'node:assert/strict'
import { limitConcurrency, rollUp, summariseChecks } from '../src/main/providers/types.ts'
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
