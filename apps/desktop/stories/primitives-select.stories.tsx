import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { SelectMenu, ThemeProvider } from '@qrtz/ui';

/** The theme select on Radix Select (shadcn Select). screenshots/primitives.spec.ts opens it, screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/select' };
export default meta;

function Demo() {
  const [theme, setTheme] = useState('dark');
  return (
    <SelectMenu
      aria-label="theme"
      value={theme}
      onValueChange={setTheme}
      options={[
        { value: 'dark', label: 'dark' },
        { value: 'light', label: 'light' },
        { value: 'system', label: 'system' },
      ]}
    />
  );
}

export const Theme: StoryObj = {
  name: 'theme',
  render: () => (
    <ThemeProvider mode="dark">
      <Demo />
    </ThemeProvider>
  ),
};
