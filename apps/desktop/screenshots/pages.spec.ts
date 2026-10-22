import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join } from 'node:path';

type Entry = { id: string; title: string; type: string };
const index = JSON.parse(
  readFileSync(
    join(import.meta.dirname, '..', 'storybook-static', 'index.json'),
    'utf8',
  ),
) as { entries: Record<string, Entry> };
const stories = Object.values(index.entries).filter((e) => e.type === 'story');
const pages = stories.filter((s) => s.id.startsWith('pages--'));
const checks = stories.filter((s) => s.id.startsWith('checks--'));

test('every page has a story', () => {
  expect(pages.length).toBeGreaterThanOrEqual(12);
  expect(checks.length).toBeGreaterThanOrEqual(1);
});

async function open(
  page: import('@playwright/test').Page,
  id: string,
  ready = '[data-page]',
) {
  await page.goto(`/iframe.html?id=${id}&viewMode=story`);
  await page.locator(ready).first().waitFor();
  await page.waitForLoadState('networkidle');
  await page.evaluate(() => document.fonts.ready);
}

for (const s of pages) {
  test(s.id, async ({ page }, info) => {
    await open(page, s.id);
    // Always keep the run's own screenshot, pass or fail, so runner font differences can be inspected.
    await page.screenshot({ path: info.outputPath(`${s.id}.png`) });
    // Tolerance: up to 1500 pixels may differ by more than the 0.2 colour threshold (font antialiasing across
    // Linux images). A changed label or a missing icon is more than that.
    await expect(page).toHaveScreenshot(`${s.id}.png`, {
      maxDiffPixels: 1500,
      threshold: 0.2,
    });
  });
}

for (const s of checks) {
  test(`${s.id}: nothing overflows sideways`, async ({ page }, info) => {
    await page.setViewportSize({ width: 1100, height: 720 });
    await open(page, s.id, '#storybook-root > *');
    await page.screenshot({
      path: info.outputPath(`${s.id}.png`),
      fullPage: true,
    });
    // Every block that doesn't clip or scroll its own content must fit its box, and the page must not scroll
    // sideways. Truncating (overflow hidden + ellipsis) and scrolling boxes are allowed.
    const spills = await page.evaluate(() => {
      const root = document.querySelector('#storybook-root')!;
      const out: string[] = [];
      for (const el of [root, ...root.querySelectorAll('*')]) {
        const cs = getComputedStyle(el);
        if (cs.display === 'inline' || cs.display === 'contents') continue;
        if (cs.overflowX !== 'visible') continue;
        if (el.scrollWidth > el.clientWidth + 1)
          out.push(
            `${el.tagName.toLowerCase()}.${el.className}: ${el.scrollWidth} > ${el.clientWidth}`,
          );
      }
      const doc = document.scrollingElement!;
      if (doc.scrollWidth > doc.clientWidth + 1)
        out.push(`page: ${doc.scrollWidth} > ${doc.clientWidth}`);
      return out;
    });
    expect(spills).toEqual([]);
  });
}

// Echo's history entries: right-click opens the ActionMenu (copy, delete). Screenshot with it open, and an axe pass
// (closed and open) with no serious or critical violations.
const axeSource = readFileSync(
  createRequire(import.meta.url).resolve('axe-core/axe.min.js'),
  'utf8',
);
type AxeWindow = {
  axe: {
    run: (
      c: unknown,
      o: unknown,
    ) => Promise<{
      violations: { id: string; impact: string; nodes: unknown[] }[];
    }>;
  };
};
test('pages--echo: entry action menu', async ({ page }, info) => {
  await open(page, 'pages--echo');
  await page.addScriptTag({ content: axeSource });
  const axe = () =>
    page.evaluate(async () => {
      const r = await (window as unknown as AxeWindow).axe.run(document, {
        rules: { region: { enabled: false } },
      });
      return r.violations
        .filter((v) => v.impact === 'serious' || v.impact === 'critical')
        .map((v) => `${v.id} (${v.nodes.length})`);
    });
  expect(await axe()).toEqual([]);
  const entry = page.getByTestId('talk-entry').first();
  await entry.click({ button: 'right', position: { x: 60, y: 12 } });
  await page.getByTestId('talk-entry-menu').waitFor();
  await expect(page.getByTestId('talk-entry-copy')).toBeFocused();
  await page.screenshot({
    path: info.outputPath('pages--echo-action-menu.png'),
  });
  await expect(page).toHaveScreenshot('pages--echo-action-menu.png', {
    maxDiffPixels: 1500,
    threshold: 0.2,
  });
  expect(await axe()).toEqual([]);
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('talk-entry-menu')).toHaveCount(0);
  await expect(entry).toBeFocused();
});
