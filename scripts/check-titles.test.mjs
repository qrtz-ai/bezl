import { describe, expect, it } from 'vitest';
import { titleProblems } from './check-titles.mjs';

describe('check-titles', () => {
  it('flags uppercase titles and allows lowercase, symbols and user content', () => {
    expect(titleProblems('<Heading>Settings</Heading>', 'x.tsx')).toHaveLength(
      1,
    );
    expect(titleProblems('<Heading>settings ⌘K</Heading>', 'x.tsx')).toEqual(
      [],
    );
    expect(
      titleProblems('<Heading preserveCase>Weekly Sync</Heading>', 'x.tsx'),
    ).toEqual([]);
    expect(
      titleProblems('<Heading level={2}>{meeting.title}</Heading>', 'x.tsx'),
    ).toEqual([]);
    expect(
      titleProblems('<SettingsSection title="Privacy">', 'x.tsx'),
    ).toHaveLength(1);
    expect(
      titleProblems('<ConfirmDialog\n  open\n  title="Delete?"', 'x.tsx'),
    ).toHaveLength(1);
    expect(
      titleProblems("name: 'Note',", 'packages/features/note/src/manifest.ts'),
    ).toHaveLength(1);
    expect(
      titleProblems("title: 'qrtz HUD',", 'apps/desktop/src/main/index.ts'),
    ).toHaveLength(1);
    expect(
      titleProblems(
        "{ label: 'Quit qrtz', action: 'quit' }",
        'apps/desktop/src/main/tray-model.ts',
      ),
    ).toHaveLength(1);
    expect(titleProblems('<Text>Body Text Stays</Text>', 'x.tsx')).toEqual([]);
  });
});
