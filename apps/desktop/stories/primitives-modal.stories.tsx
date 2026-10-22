import type { Meta, StoryObj } from '@storybook/react-vite';
import { Modal, Text, ThemeProvider } from '@qrtz/ui';

/** Modal on Radix Dialog (shadcn Dialog). screenshots/primitives.spec.ts screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/modal' };
export default meta;

export const Open: StoryObj = {
  name: 'open',
  render: () => (
    <ThemeProvider mode="dark">
      <Modal open onClose={() => {}} title="about qrtz">
        <Text>modals trap focus and close on esc.</Text>
      </Modal>
    </ThemeProvider>
  ),
};
