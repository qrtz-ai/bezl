import { _electron as electron, expect, test } from '@playwright/test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// Shell UX: ⌘K command palette (navigate + record) and the component gallery (M6.1/M6.2).
const appDir = path.join(__dirname, '..', 'apps', 'desktop');
const shots = path.join(__dirname, '..', 'test-results');

test('command palette and component gallery', async () => {
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    QRTZ_USER_DATA: fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-shell-')),
    QRTZ_MOCK_HELPER: '1',
    QRTZ_DEVTOOLS: '0',
  };
  const app = await electron.launch({
    args: [appDir, ...(process.platform === 'linux' ? ['--no-sandbox'] : [])],
    env,
  });
  const win = await app.firstWindow();
  const errors: string[] = [];
  win.on('pageerror', (e) => errors.push(e.message));
  await expect(win.getByTestId('nav-note')).toHaveAttribute(
    'aria-current',
    'page',
  );

  // First run: Note is the home screen, an empty meeting list and one Record button.
  await expect(win.getByTestId('note-app')).toBeVisible();
  await expect(win.getByTestId('empty-state').first()).toContainText(
    'No meetings',
  );
  await expect(win.getByTestId('note-record-manual')).toBeEnabled();
  await expect(win.getByTestId('nav-note')).toHaveText('note');
  // Icons are Feather SVGs, never glyph characters.
  await expect(
    win.getByTestId('note-record-manual').locator('svg[data-icon="circle"]'),
  ).toHaveCount(1);
  await expect(
    win.getByTestId('note-search-toggle').locator('svg[data-icon="search"]'),
  ).toHaveCount(1);
  expect(await win.locator('body').innerText()).not.toMatch(/[■▶❚⌕↻✎☐●◎⚙▦]/);
  await win.screenshot({ path: path.join(shots, 'note-first-run.png') });

  // ⌘K / Ctrl+K opens the palette; fuzzy filter + Enter navigates.
  await win.keyboard.press('ControlOrMeta+k');
  await expect(win.getByTestId('palette')).toBeVisible();
  await expect(win.getByTestId('cmd-note.start')).toBeVisible();
  await win.screenshot({ path: path.join(shots, 'palette.png') });
  await win.getByTestId('palette-input').fill('gallery');
  await expect(win.getByTestId('cmd-nav.gallery')).toBeVisible();
  await expect(win.getByTestId('cmd-nav.settings')).toHaveCount(0);
  await win.keyboard.press('Enter');
  await expect(win.getByTestId('palette')).toHaveCount(0);

  // Gallery renders every section; dialog and toast work.
  for (const id of [
    'color',
    'type',
    'buttons',
    'badges',
    'inputs',
    'nav',
    'overlays',
    'empty',
  ])
    await expect(win.getByTestId(`gallery-${id}`)).toBeVisible();
  await win.screenshot({
    path: path.join(shots, 'gallery.png'),
    fullPage: true,
  });
  await win.getByTestId('gallery-open-dialog').click();
  await expect(
    win.getByRole('dialog', { name: 'Delete meeting?' }),
  ).toBeVisible();
  await win.keyboard.press('Escape');
  await expect(win.getByRole('dialog')).toHaveCount(0);
  await win.getByTestId('gallery-toast').click();
  await expect(win.getByTestId('toast')).toContainText('Saved');
  await win.getByRole('switch').first().click();
  await expect(win.getByRole('switch').first()).toHaveAttribute(
    'aria-checked',
    'false',
  );

  // Icons page: every Feather icon by name, rendered as SVG at token sizes.
  await win
    .getByTestId('gallery-page')
    .getByRole('tab', { name: 'Icons' })
    .click();
  await expect(win.getByTestId('gallery-icons')).toBeVisible();
  await expect(win.getByTestId('gallery-icon-mic').locator('svg')).toHaveCount(
    1,
  );
  expect(
    await win.getByTestId('gallery-icons').locator('svg[data-icon]').count(),
  ).toBeGreaterThan(15);
  await expect(win.getByTestId('gallery-buttons')).toHaveCount(0);
  await win.screenshot({ path: path.join(shots, 'gallery-icons.png') });
  await win
    .getByTestId('gallery-page')
    .getByRole('tab', { name: 'Components' })
    .click();
  await expect(win.getByTestId('gallery-buttons')).toBeVisible();

  // Palette (⌘K) starts and stops a recording through shell-scoped IPC.
  await win.keyboard.press('ControlOrMeta+k');
  await win.getByTestId('palette-input').fill('start rec');
  await win.keyboard.press('Enter');
  await expect(win.getByTestId('note-hud')).toBeVisible({ timeout: 10_000 });
  await win.keyboard.press('ControlOrMeta+k');
  await expect(win.getByTestId('cmd-note.stop')).toBeVisible();
  await win.getByTestId('cmd-note.stop').click();
  await expect(win.getByTestId('toast').last()).toContainText(
    'Recording stopped',
  );

  expect(errors).toEqual([]);
  await app.close();
});
