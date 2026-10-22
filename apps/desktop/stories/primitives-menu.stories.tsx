import type { Meta, StoryObj } from '@storybook/react-vite';
import { Menu, ThemeProvider } from '@qrtz/ui';

/** Menu on Radix DropdownMenu (shadcn DropdownMenu). screenshots/primitives.spec.ts opens it, screenshots it and runs axe. */
const meta: Meta = { title: 'primitives/menu' };
export default meta;

export const Actions: StoryObj = {
  name: 'actions',
  render: () => (
    <ThemeProvider mode="dark">
      <Menu
        label="more actions for echo"
        items={[
          { id: 'opts', label: 'opts', icon: 'settings', onSelect: () => {} },
          { id: 'off', label: 'turn off', disabled: true, onSelect: () => {} },
          { id: 'rm', label: 'remove', danger: true, onSelect: () => {} },
        ]}
      />
    </ThemeProvider>
  ),
};
