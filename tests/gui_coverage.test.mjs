import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import test from 'node:test';
import { operationLabels } from '../ui/src/shared_admin_state.ts';

const coverage = new URL('./browser/coverage.md', import.meta.url);

test('Hub GUI audit lists each public shared administration operation once', async () => {
  const doc = await readFile(coverage, 'utf8');
  const operations = [...doc.matchAll(/^\| `([a-z]+_[a-z_]+)` \|/gm)].map(match => match[1]);
  assert.deepEqual(operations.sort(), Object.keys(operationLabels).sort());
});

test('Hub GUI audit references all current browser specs without retired entries', async () => {
  const doc = await readFile(coverage, 'utf8');
  const listed = [...doc.matchAll(/^\| `([a-z-]+\.spec\.mjs)` \|/gm)].map(match => match[1]);
  const current = (await readdir(new URL('./browser/', import.meta.url))).filter(name => name.endsWith('.spec.mjs'));
  assert.deepEqual(listed.sort(), current.sort());
});
