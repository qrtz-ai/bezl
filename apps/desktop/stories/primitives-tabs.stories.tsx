import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { Tabs, ThemeProvider } from '@qrtz/ui';

/** Tabs on Radix Tabs (shadcn Tabs). screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/tabs' };
export default meta;

function Demo() {
  const [tab, setTab] = useState<'transcript' | 'summary' | 'notes'>('summary');
  return (
    <Tabs
      tabs={[
        { id: 'transcript', label: 'transcript' },
        { id: 'summary', label: 'summary' },
        { id: 'notes', label: 'notes' },
      ]}
      value={tab}
      onChange={setTab}
    />
  );
}

export const Row: StoryObj = {
  name: 'row',
  render: () => (
    <ThemeProvider mode="dark">
      <Demo />
    </ThemeProvider>
  ),
};
