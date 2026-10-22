import type { Meta, StoryObj } from '@storybook/react-vite';
import { Input, Stack, ThemeProvider } from '@qrtz/ui';

/** Input on shadcn Input: empty, filled and disabled. screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/input' };
export default meta;

export const States: StoryObj = {
  name: 'states',
  render: () => (
    <ThemeProvider mode="dark">
      <Stack gap={3} pad={4}>
        <Input aria-label="search" placeholder="search" />
        <Input aria-label="name" defaultValue="qrtz" />
        <Input aria-label="locked" defaultValue="locked" disabled />
      </Stack>
    </ThemeProvider>
  ),
};
