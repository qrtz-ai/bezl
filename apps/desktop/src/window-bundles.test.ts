import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { build, type Rolldown } from 'vite';

// ADR 0090: the HUD is its own page (echo's pill is native, ADR 0092). Its bundle must not pull in the shell, its
// router or any sub app, and every page carries the same dev CSP.
const root = join(dirname(fileURLToPath(import.meta.url)), '..');

type Chunk = Rolldown.OutputChunk;

/** Every chunk a page loads before it can render: the entry and its static imports. */
function closure(
  chunks: Map<string, Chunk>,
  entry: Chunk,
  lazy: boolean | ((c: Chunk) => boolean) = false,
): Chunk[] {
  const seen = new Map<string, Chunk>();
  const walk = (c: Chunk) => {
    if (seen.has(c.fileName)) return;
    seen.set(c.fileName, c);
    for (const i of c.imports) walk(chunks.get(i)!);
    for (const i of c.dynamicImports) {
      const d = chunks.get(i)!;
      if (lazy === true || (lazy && lazy(d))) walk(d);
    }
  };
  walk(entry);
  return [...seen.values()];
}

const SHELL_ONLY =
  /packages\/features\/|src\/shell\/(App|SubAppHost|subapps|Palette|Onboarding)\.tsx?$|src\/renderer\.tsx$|node_modules\/react-router/;

describe('window pages', () => {
  it('the hud bundle includes no shell, router or sub app modules', async () => {
    // A production build, as `vite build` makes it (vitest sets NODE_ENV=test, which keeps DEV-only code).
    const env = process.env.NODE_ENV;
    process.env.NODE_ENV = 'production';
    const out = (await build({
      root,
      mode: 'production',
      configFile: join(root, 'vite.config.mts'),
      logLevel: 'silent',
      build: { write: false },
    }).finally(() => {
      process.env.NODE_ENV = env;
    })) as Rolldown.RolldownOutput;
    const chunks = new Map(
      out.output
        .filter((o): o is Chunk => o.type === 'chunk')
        .map((c) => [c.fileName, c]),
    );
    const entry = (name: string) =>
      [...chunks.values()].find((c) => c.isEntry && c.name === name)!;
    const sizes: Record<string, number> = {};
    for (const name of ['hud', 'main']) {
      const loaded = closure(chunks, entry(name));
      sizes[name] = loaded.reduce((n, c) => n + c.code.length, 0);
      if (name === 'main') continue;
      const modules = loaded.flatMap((c) => c.moduleIds);
      expect(modules.some((m) => m.endsWith('src/hud-main.tsx'))).toBe(true);
      expect(modules.filter((m) => SHELL_ONLY.test(m))).toEqual([]);
      // Nor can it lazily reach a sub app.
      expect(loaded.flatMap((c) => c.dynamicImports)).toEqual([]);
    }
    // The main page still reaches the shell and its sub apps (lazily).
    const main = closure(chunks, entry('main'), true);
    expect(
      main.flatMap((c) => c.moduleIds).some((m) => /src\/shell\/App/.test(m)),
    ).toBe(true);
    expect(
      main
        .flatMap((c) => c.moduleIds)
        .some((m) => /packages\/features\/note\//.test(m)),
    ).toBe(true);
    // ADR 0093: first paint of the main window is the shell and the note list. The meeting detail, the
    // palette, onboarding, settings and the gallery are their own chunks, loaded when needed.
    const first = closure(chunks, entry('main'), (c) =>
      c.moduleIds.some((m) =>
        /src\/renderer\.tsx$|note\/src\/NoteView\.tsx$/.test(m),
      ),
    ).flatMap((c) => c.moduleIds);
    expect(first.some((m) => m.endsWith('note/src/NoteView.tsx'))).toBe(true);
    const LATER =
      /note\/src\/(MeetingDetail|Speakers)\.tsx$|shell\/(Palette|Onboarding)\.tsx$|features\/(settings|gallery|talk)\/src\/\w+View\.tsx$|ui\/src\/gallery\//;
    expect(first.filter((m) => LATER.test(m))).toEqual([]);
    for (const later of ['MeetingDetail', 'Palette', 'Onboarding'])
      expect(
        main.flatMap((c) => c.moduleIds).some((m) => m.includes(later)),
      ).toBe(true);
    // No pill page any more: echo's pill is drawn by the helper (ADR 0092).
    expect(chunks.has('pill.html')).toBe(false);
    expect(
      [...chunks.values()].some((c) => c.isEntry && c.name === 'pill'),
    ).toBe(false);
    console.info('[window-bundles] js bytes loaded per page', sizes);
  }, 60_000);

  it('hud.html carries the same CSP as index.html', () => {
    const csp = (f: string) =>
      /http-equiv="Content-Security-Policy"\s+content="([^"]+)"/.exec(
        readFileSync(join(root, f), 'utf8'),
      )?.[1];
    expect(csp('index.html')).toBeTruthy();
    expect(csp('hud.html')).toBe(csp('index.html'));
  });
});
