import { describe, expect, it } from 'vitest';
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { checkBoundaries } from './check-boundaries.mjs';

function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), 'qrtz-b-'));
  for (const [p, c] of Object.entries(files)) {
    mkdirSync(join(root, p, '..'), { recursive: true });
    writeFileSync(join(root, p), c);
  }
  return root;
}
const pkg = (deps = {}) => JSON.stringify({ dependencies: deps });

describe('check-boundaries', () => {
  it('passes a clean repo', () => {
    const root = fixture({
      'packages/core/package.json': pkg(),
      'packages/core/src/a.ts': "import { z } from 'zod';",
      'packages/features/a/package.json': pkg({ '@qrtz/core': '*' }),
      'packages/features/a/src/i.ts': "import { x } from '@qrtz/core';",
    });
    expect(checkBoundaries(root).errors).toEqual([]);
  });
  it('catches electron/node in features, cross-feature imports and node in core', () => {
    const root = fixture({
      'packages/core/package.json': pkg(),
      'packages/core/src/a.ts': "import fs from 'node:fs';",
      'packages/features/a/package.json': pkg({ '@qrtz/feature-b': '*' }),
      'packages/features/a/src/i.ts':
        "import { ipcRenderer } from 'electron';\nimport b from '@qrtz/feature-b';",
      'apps/desktop/package.json': pkg(),
      'apps/desktop/src/shell/App.tsx':
        "import { ipcRenderer } from 'electron';",
    });
    const { errors } = checkBoundaries(root);
    expect(errors.join('\n')).toMatch(
      /core\/src\/a.ts imports banned module node:fs/,
    );
    expect(errors.join('\n')).toMatch(
      /features\/a\/src\/i.ts imports banned module electron/,
    );
    expect(errors.join('\n')).toMatch(/imports @qrtz\/feature-b/);
    expect(errors.join('\n')).toMatch(
      /package.json depends on @qrtz\/feature-b/,
    );
    expect(errors.join('\n')).toMatch(
      /shell\/App.tsx imports banned module electron/,
    );
  });
});
