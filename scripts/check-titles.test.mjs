import { describe, expect, it } from 'vitest';
import { fixSource, titleProblems } from './check-titles.mjs';

const n = (src, rel = 'x.tsx') => titleProblems(src, rel).length;

describe('check-titles (all UI chrome lowercase)', () => {
  it('flags headings, buttons, labels, placeholders, options, menus, toasts', () => {
    expect(n('const a = <Heading>Settings</Heading>;')).toBe(1);
    expect(n('const a = <Button>Retry</Button>;')).toBe(1);
    expect(n('const a = <Field label="API key" />;')).toBe(1);
    expect(n('const a = <Input placeholder="Search" />;')).toBe(1);
    expect(n('const a = <option value="x">OpenAI</option>;')).toBe(1);
    expect(n("const a = <B label={busy ? 'Summarizing' : 'Re-run'} />;")).toBe(
      2,
    );
    expect(n("const a = <B>{busy ? 'Stop' : 'Dictate'}</B>;")).toBe(2);
    expect(n("const m = [{ label: 'Copy Markdown', onSelect }];", 'x.ts')).toBe(
      1,
    );
    expect(n("toast('Copied', 'success');", 'x.ts')).toBe(1);
    expect(
      n(
        "export const m = { name: 'Note' };",
        'packages/features/note/src/manifest.ts',
      ),
    ).toBe(1);
  });
  it('allows lowercase, shortcut glyphs, user content, code values and ignores', () => {
    expect(n('const a = <Heading>settings ⌘K</Heading>;')).toBe(0);
    expect(n('const a = <Heading>{meeting.title}</Heading>;')).toBe(0);
    expect(n('const a = <Kbd>⌘⇧R</Kbd>;')).toBe(0);
    expect(
      n("const a = <B data-testid=\"X\" variant={k === 'A' ? 'x' : 'y'} />;"),
    ).toBe(0);
    expect(
      n('const a = { title: `Sync #${i}` }; // check-titles-ignore', 'x.ts'),
    ).toBe(0);
  });
  it('--fix lowercases only the flagged literals', () => {
    const src =
      'const a = <B label="Copy" data-testid="Keep">Hello {name}</B>;';
    expect(fixSource(src, titleProblems(src, 'x.tsx'))).toBe(
      'const a = <B label="copy" data-testid="Keep">hello {name}</B>;',
    );
  });
});
