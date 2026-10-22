import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { Switch, ThemeProvider } from '@qrtz/ui';

/** Switch (the ctrl toggle) on Radix Switch (shadcn Switch). screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/switch' };
export default meta;

function Demo() {
  const [on, setOn] = useState(true);
  const [off, setOff] = useState(false);
  return (
    <>
      <Switch checked={on} onChange={setOn} label="ctrl" showLabel />
      <Switch checked={off} onChange={setOff} label="echo" showLabel />
      <Switch checked={false} onChange={() => {}} label="nano" disabled />
    </>
  );
}

export const States: StoryObj = {
  name: 'states',
  render: () => (
    <ThemeProvider mode="dark">
      <Demo />
    </ThemeProvider>
  ),
};
