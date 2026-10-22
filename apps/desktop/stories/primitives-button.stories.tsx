import type { Meta, StoryObj } from '@storybook/react-vite';
import { Button, Stack, ThemeProvider } from '@qrtz/ui';

/** Button on shadcn Button: every variant and size. screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/button' };
export default meta;

export const Variants: StoryObj = {
  name: 'variants',
  render: () => (
    <ThemeProvider mode="dark">
      <Stack gap={3} pad={4}>
        <Stack dir="row" gap={2}>
          <Button variant="primary">save</Button>
          <Button variant="secondary" icon="download">
            download
          </Button>
          <Button variant="ghost">cancel</Button>
          <Button variant="danger" icon="trash">
            delete
          </Button>
        </Stack>
        <Stack dir="row" gap={2}>
          <Button size="sm">small</Button>
          <Button size="md">medium</Button>
          <Button size="lg">large</Button>
          <Button disabled>disabled</Button>
          <Button loading>loading</Button>
          <Button variant="ghost" size="sm" icon="close" aria-label="clear" />
        </Stack>
      </Stack>
    </ThemeProvider>
  ),
};
