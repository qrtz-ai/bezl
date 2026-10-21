// QRT-292: module-code.sh holds nano, snap and ctrl's module.json to the identifier and team pin, and refuses any
// or / || / not / ! clause however it is spaced, quoted or cased (`"T"or anchor apple` must not pass).
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';

const lib = path.join(import.meta.dirname, 'module-code.sh');
const modules = path.join(import.meta.dirname, '../../modules');

const run = (fn, ...args) => {
  try {
    execFileSync('bash', ['-c', `. "$1"; shift; ${fn} "$@"`, 'x', lib, ...args], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    return true;
  } catch {
    return false;
  }
};

const T = 'ABCDE12345';
const pin = `certificate leaf[subject.OU] = "${T}"`;
const good = `identifier "ai.qrtz.p" and anchor apple generic and ${pin}`;
const manifest = (req, extra = {}) => {
  const f = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'mc-')), 'module.json');
  fs.writeFileSync(f, JSON.stringify({ code: { bundleId: 'ai.qrtz.p', designatedRequirement: req }, ...extra }));
  return f;
};

describe('scripts/modules/module-code.sh module_code', () => {
  for (const mod of ['nano', 'snap', 'ctrl']) {
    it(`${mod}: passes with qrtz's team, fails for another`, () => {
      const m = path.join(modules, mod, 'module.json');
      expect(run('module_code', m, 'MR26CCZ496')).toBe(true);
      expect(run('module_code', m, 'ZZZZZ99999')).toBe(false);
    });
  }

  it('passes a requirement that pins identifier and team with only and', () => {
    expect(run('module_code', manifest(good), T)).toBe(true);
    expect(run('module_code', manifest(`identifier "ai.qrtz.p" and ${pin}`), T)).toBe(true);
    // a keyword inside a string literal is data, not an operator
    expect(
      run('module_code', manifest(`identifier "ai.qrtz.p" and ${pin} and info[CFBundleName] = "or not !"`), T),
    ).toBe(true);
  });

  const bypasses = {
    'quote then or': `identifier "ai.qrtz.p" and ${pin}or anchor apple`,
    'quote then or, Bozniak case': `identifier "ai.qrtz.p" and certificate leaf[subject.OU] = "${T}"or anchor apple`,
    'or then quote': `identifier "ai.qrtz.p" and anchor apple or"x" = "x" and ${pin}`,
    'quote then not': `identifier "ai.qrtz.p" and ${pin} and "x"not anchor apple`,
    'paren then or': `identifier "ai.qrtz.p" and (${pin})or(anchor apple)`,
    'or then paren': `identifier "ai.qrtz.p" and ${pin} or(anchor apple)`,
    'spaced or': `${good} or anchor apple`,
    'tab-free double space or': `${good}  or  anchor apple`,
    'uppercase OR': `${good} OR anchor apple`,
    'mixed case Not': `identifier "ai.qrtz.p" and Not ${pin}`,
    '|| spaced': `${good} || anchor apple`,
    '|| glued to quote': `identifier "ai.qrtz.p" and ${pin}||anchor apple`,
    'single |': `${good} | anchor apple`,
    'not spaced': `identifier "ai.qrtz.p" and not ${pin}`,
    '! spaced': `identifier "ai.qrtz.p" and ! ${pin}`,
    '! glued': `identifier "ai.qrtz.p" and !${pin}`,
    '! glued to paren': `identifier "ai.qrtz.p" and !(${pin})`,
    'quote then !': `identifier "ai.qrtz.p" and ${pin} and "x"!anchor apple`,
    'or inside parens': `identifier "ai.qrtz.p" and (${pin} or identifier "ai.qrtz.p")`,
    'leading or': `anchor apple or identifier "ai.qrtz.p" and ${pin}`,
    'escaped quote': `identifier "ai.qrtz.p" and ${pin} and info[x] = "a\\" or anchor apple \\""`,
    'unbalanced quote': `identifier "ai.qrtz.p" and ${pin} and "x or anchor apple`,
  };
  for (const [name, req] of Object.entries(bypasses)) {
    it(`refuses: ${name}`, () => {
      expect(run('module_code', manifest(req), T)).toBe(false);
    });
  }

  it('refuses no team pin, wrong identifier, tab, newline, bad JSON, missing file, bad team arg', () => {
    expect(run('module_code', manifest('identifier "ai.qrtz.p" and anchor apple generic'), T)).toBe(false);
    expect(run('module_code', manifest(`identifier "ai.qrtz.q" and ${pin}`), T)).toBe(false);
    expect(run('module_code', manifest(`identifier "ai.qrtz.p" and\t${pin}`), T)).toBe(false);
    expect(run('module_code', manifest(`identifier "ai.qrtz.p" and\n${pin}`), T)).toBe(false);
    const bad = manifest(good);
    fs.writeFileSync(bad, '{"code":{"bundleId":"ai.qrtz.p",');
    expect(run('module_code', bad, T)).toBe(false);
    expect(run('module_code', path.join(os.tmpdir(), 'no-such-module.json'), T)).toBe(false);
    expect(run('module_code', manifest(good), '')).toBe(false);
  });
});

describe.runIf(process.platform === 'darwin')('scripts/modules/module-code.sh check_package (macOS)', () => {
  it('refuses a file-name identifier, a table mismatch, an unpinned team, and ad-hoc under Developer ID', () => {
    const m = manifest(`identifier "ai.qrtz.p" and ${pin}`);
    const exe = path.join(path.dirname(m), 'qrtz-p');
    fs.copyFileSync('/usr/bin/true', exe);
    const sign = (id) => execFileSync('codesign', ['--force', '--identifier', id, '--sign', '-', exe]);
    sign('qrtz-p');
    expect(run('check_package', exe, m, 'ai.qrtz.p', T, '-')).toBe(false);
    sign('ai.qrtz.p');
    expect(run('check_package', exe, m, 'ai.qrtz.p', T, '-')).toBe(true);
    expect(run('check_package', exe, m, 'ai.qrtz.other', T, '-')).toBe(false);
    expect(run('check_package', exe, m, 'ai.qrtz.p', 'ZZZZZ99999', '-')).toBe(false);
    expect(run('check_package', exe, m, 'ai.qrtz.p', T, `Developer ID Application: Test (${T})`)).toBe(false);
  });
});
