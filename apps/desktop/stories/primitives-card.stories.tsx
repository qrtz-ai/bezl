import type { Meta, StoryObj } from '@storybook/react-vite';
import { Card, Stack, Text, ThemeProvider } from '@qrtz/ui';

/** Card on shadcn Card: paddings, interactive and selected. screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/card' };
export default meta;

export const States: StoryObj = {
  name: 'states',
  render: () => (
    <ThemeProvider mode="dark">
      <Stack gap={3} pad={4}>
        <Card padding={3}>
          <Text>compact</Text>
        </Card>
        <Card padding={4} interactive>
          <Text>interactive</Text>
        </Card>
        <Card selected>
          <Text>selected</Text>
        </Card>
      </Stack>
    </ThemeProvider>
  ),
};
