import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { HelperToHost, HostToHelper, parseHelperLine } from './protocol';

// Shared contract with the Swift helper: native/qrtz-helper/Tests/QrtzSharedTests/ProtocolGoldenTests.swift
// round-trips these same files byte-for-byte.
const dir = path.resolve(
  __dirname,
  '../../../../native/qrtz-helper/Fixtures/protocol',
);
const lines = (f: string) =>
  fs.readFileSync(path.join(dir, f), 'utf8').split('\n').filter(Boolean);

describe('helper protocol golden fixtures', () => {
  it('every helper→host fixture parses', () => {
    for (const l of lines('helper-to-host.jsonl')) {
      const r = parseHelperLine(l);
      expect(r.ok, `${l} → ${r.ok ? '' : r.error}`).toBe(true);
    }
  });

  it('fixtures cover every helper→host and host→helper message type', () => {
    const typesOf = (u: { options: readonly unknown[] }) =>
      u.options.map(
        (o) => (o as { shape: { type: { value: string } } }).shape.type.value,
      );
    const seen = (f: string) =>
      new Set(lines(f).map((l) => (JSON.parse(l) as { type: string }).type));
    expect([...seen('helper-to-host.jsonl')].sort()).toEqual(
      typesOf(HelperToHost).sort(),
    );
    expect([...seen('host-to-helper.jsonl')].sort()).toEqual(
      typesOf(HostToHelper).sort(),
    );
  });

  it('every host→helper fixture validates', () => {
    for (const l of lines('host-to-helper.jsonl')) {
      expect(HostToHelper.safeParse(JSON.parse(l)).success, l).toBe(true);
    }
  });

  it('invalid fixtures are rejected', () => {
    for (const l of lines('invalid-helper-to-host.jsonl')) {
      expect(parseHelperLine(l).ok, l).toBe(false);
    }
  });
});
