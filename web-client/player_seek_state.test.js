import assert from 'node:assert/strict';
import test from 'node:test';

import { createSeekState } from './player_seek_state.js';

test('keeps the requested seek time visible while playback still reports the old time', () => {
  const seekState = createSeekState();

  seekState.update(72);

  assert.equal(seekState.getDisplayTime(12), 72);
  assert.equal(seekState.commit(12), 72);
  assert.equal(seekState.getDisplayTime(12), 72);
});

test('clears the requested seek time after playback reaches the target', () => {
  const seekState = createSeekState();

  seekState.update(72);
  seekState.commit(12);

  assert.equal(seekState.settle(72.2), true);
  assert.equal(seekState.getDisplayTime(72.2), 72.2);
});

test('uses the release time when a seek is committed without a prior drag preview', () => {
  const seekState = createSeekState();

  assert.equal(seekState.commit(45), 45);
  assert.equal(seekState.getDisplayTime(10), 45);
});
