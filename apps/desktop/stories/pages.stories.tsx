import type { Meta, StoryObj } from '@storybook/react-vite';
import { loadRoutes, Page } from './mount';

/** Every page of the main window, on the desktop mock backend. Each story is one route. */
const meta: Meta<typeof Page> = {
  title: 'pages',
  component: Page,
  loaders: [async () => ({ routes: await loadRoutes() })],
  render: (args, { loaded }) => (
    <Page routes={loaded.routes} path={args.path} />
  ),
};
export default meta;
type Story = StoryObj<typeof Page>;

const at = (path: string): Story => ({ args: { path } });

export const Dash = at('/dash');
export const Snap = at('/snap');
export const SnapOpts = at('/snap/settings');
export const Echo = at('/talk');
export const EchoOpts = at('/talk/settings');
export const Ctrl = at('/ctrl');
export const CtrlOpts = at('/ctrl/settings');
export const Nano = at('/note');
export const NanoOpts = at('/note/settings');
export const Qrtz = at('/qrtz');
export const QrtzItem = at('/qrtz/item/echo');
export const Opts = at('/qrtz/settings');
