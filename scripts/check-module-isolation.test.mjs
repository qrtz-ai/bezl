import { describe, expect, it } from 'vitest';
import { check, checkRepo } from './check-module-isolation.mjs';

const toml = (deps) =>
  `[workspace]\n\n[workspace.dependencies]\n${deps}\n\n[profile.release]\nlto = true\n`;

describe('check-module-isolation', () => {
  it('passes repo-wide entries', () => {
    const pkg = { scripts: { lint: 'x' }, devDependencies: { vitest: '1' } };
    expect(
      check(pkg, toml('serde = "1"\nqrtz-x = { path = "crates/x" }'), {
        serde: 2,
      }),
    ).toEqual([]);
  });
  it('flags module scripts, module deps and single-user workspace crates', () => {
    const pkg = { scripts: { 'dash:dev': 'x' }, dependencies: { lodash: '1' } };
    expect(check(pkg, toml('once = "1"'), { once: 1 })).toHaveLength(3);
  });
  it('holds for this repo', () => {
    expect(checkRepo()).toEqual([]);
  });
});
