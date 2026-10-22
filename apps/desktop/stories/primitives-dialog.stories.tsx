import type { Meta, StoryObj } from '@storybook/react-vite';
import { Button, Dialog, Stack, Text, ThemeProvider } from '@qrtz/ui';

/** Dialog (consent/confirm) on shadcn Dialog. screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/dialog' };
export default meta;

export const Confirm: StoryObj = {
  name: 'confirm',
  render: () => (
    <ThemeProvider mode="dark">
      <Dialog open onClose={() => {}} title="delete meeting?">
        <Stack gap={4} pad={5}>
          <Text>this removes its transcript and notes.</Text>
          <Button variant="danger">delete</Button>
        </Stack>
      </Dialog>
    </ThemeProvider>
  ),
};
