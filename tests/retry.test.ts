import test from 'node:test'
import assert from 'node:assert'
import { errorToMessage, loadWithRetry } from '../src/core/retry.ts'

const noSleep = async () => {}

test('succeeds after transient "state not managed" failures (startup race)', async () => {
  let calls = 0
  const result = await loadWithRetry(
    async () => {
      calls++
      if (calls < 4) throw 'state not managed for field `config_manager` on command `get_config`'
      return { ok: true }
    },
    { attempts: 20, delayMs: 500, sleep: noSleep },
  )
  assert.deepStrictEqual(result, { ok: true })
  assert.strictEqual(calls, 4)
})

test('does not retry after first success', async () => {
  let calls = 0
  await loadWithRetry(async () => { calls++; return 1 }, { attempts: 5, delayMs: 1, sleep: noSleep })
  assert.strictEqual(calls, 1)
})

test('gives up after the bounded number of attempts and rethrows the last error', async () => {
  let calls = 0
  await assert.rejects(
    loadWithRetry(
      async () => { calls++; throw new Error(`fail-${calls}`) },
      { attempts: 3, delayMs: 1, sleep: noSleep },
    ),
    /fail-3/,
  )
  assert.strictEqual(calls, 3)
})

test('waits between attempts but not after the last one', async () => {
  const sleeps: number[] = []
  await assert.rejects(
    loadWithRetry(async () => { throw new Error('x') }, {
      attempts: 3,
      delayMs: 500,
      sleep: async ms => { sleeps.push(ms) },
    }),
  )
  assert.deepStrictEqual(sleeps, [500, 500])
})

test('stops retrying once cancelled (component unmounted)', async () => {
  let calls = 0
  let cancelled = false
  await assert.rejects(
    loadWithRetry(
      async () => { calls++; cancelled = true; throw new Error('x') },
      { attempts: 10, delayMs: 1, isCancelled: () => cancelled, sleep: noSleep },
    ),
    /cancelled/,
  )
  assert.strictEqual(calls, 1)
})

test('errorToMessage handles string, Error and unknown values', () => {
  assert.strictEqual(errorToMessage('boom', 'fb'), 'boom')
  assert.strictEqual(errorToMessage(new Error('bad'), 'fb'), 'bad')
  assert.strictEqual(errorToMessage(null, 'fb'), 'fb')
  assert.strictEqual(errorToMessage({}, 'fb'), 'fb')
})
