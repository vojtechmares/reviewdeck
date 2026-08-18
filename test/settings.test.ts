import { test } from 'node:test'
import assert from 'node:assert/strict'
import { DEFAULT_SETTINGS, mergeSettings, type Settings } from '../src/shared/types.ts'

test('mergeSettings fills in everything the stored vault does not carry', () => {
  assert.deepEqual(mergeSettings({}), DEFAULT_SETTINGS)
  assert.deepEqual(mergeSettings(undefined), DEFAULT_SETTINGS)
  assert.deepEqual(mergeSettings(null), DEFAULT_SETTINGS)

  const merged = mergeSettings({ diffView: 'unified' })
  assert.equal(merged.diffView, 'unified')
  assert.equal(merged.pollInterval, DEFAULT_SETTINGS.pollInterval)
})

test('mergeSettings keeps a stored value even when it matches no default', () => {
  const merged = mergeSettings({ notificationsEnabled: false, playSound: false })
  assert.equal(merged.notificationsEnabled, false)
  assert.equal(merged.playSound, false)
})

test('mergeSettings loads a vault written before a setting was removed', () => {
  // `showWhitespace` was defined and defaulted but read nowhere, so it was removed.
  // A vault written by a build that still had it must keep loading.
  const older = { diffView: 'unified', showWhitespace: true, theme: 'dark' } as Partial<Settings>

  const merged = mergeSettings(older)

  assert.equal(merged.diffView, 'unified')
  assert.equal(merged.theme, 'dark')
  assert.equal(merged.hideApproved, DEFAULT_SETTINGS.hideApproved)
})

test('the settings type no longer carries the whitespace-display key', () => {
  assert.equal(Object.hasOwn(DEFAULT_SETTINGS, 'showWhitespace'), false)
})
