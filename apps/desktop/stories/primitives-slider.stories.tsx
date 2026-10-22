import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { Slider, ThemeProvider } from '@qrtz/ui';

/** Slider on Radix Slider (shadcn Slider). screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/slider' };
export default meta;

function Demo() {
  const [v, setV] = useState(0.4);
  return (
    <Slider value={v} min={0} max={1} label="pill opacity" onChange={setV} />
  );
}

export const Value: StoryObj = {
  name: 'value',
  render: () => (
    <ThemeProvider mode="dark">
      <Demo />
    </ThemeProvider>
  ),
};
