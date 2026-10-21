import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

type Entry = { id: string; title: string; type: string };
const index = JSON.parse(
  readFileSync(
    join(import.meta.dirname, '..', 'storybook-static', 'index.json'),
    'utf8',
  ),
) as { entries: Record<string, Entry> };
const stories = Object.values(index.entries).filter((e) => e.type === 'story');

test('every page has a story', () => {
  expect(stories.length).toBeGreaterThanOrEqual(12);
});

for (const s of stories) {
  test(s.id, async ({ page }) => {
    await page.goto(`/iframe.html?id=${s.id}&viewMode=story`);
    await page.locator('[data-page]').first().waitFor();
    await page.waitForLoadState('networkidle');
    await page.evaluate(() => document.fonts.ready);
    await expect(page).toHaveScreenshot(`${s.id}.png`, {
      maxDiffPixelRatio: 0.002,
    });
  });
}
