// QRT-354: runs stapler-retry.test.sh (fake stapler, any OS) under vitest so CI covers it without a workflow change.
import { expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

it('stapler-retry.test.sh passes', () => {
  const r = spawnSync('bash', [path.join(import.meta.dirname, 'stapler-retry.test.sh')], { encoding: 'utf8' });
  expect(r.stdout + r.stderr).toContain(' passed, 0 failed');
  expect(r.status).toBe(0);
});
