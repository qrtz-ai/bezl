import type { Meta, StoryObj } from '@storybook/react-vite';
import { Badge, Stack, ThemeProvider } from '@qrtz/ui';

/** Badge on shadcn Badge: every tone. screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/badge' };
export default meta;

export const Tones: StoryObj = {
  name: 'tones',
  render: () => (
    <ThemeProvider mode="dark">
      <Stack dir="row" gap={2} pad={4}>
        <Badge tone="neutral">neutral</Badge>
        <Badge tone="primary">new</Badge>
        <Badge tone="success">installed</Badge>
        <Badge tone="warning">update</Badge>
        <Badge tone="danger" role="alert">
          failed
        </Badge>
      </Stack>
    </ThemeProvider>
  ),
};
