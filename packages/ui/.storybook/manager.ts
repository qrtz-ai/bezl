import { addons } from 'storybook/manager-api';
import { create } from 'storybook/theming';

addons.setConfig({
  showPanel: false,
  theme: create({
    base: 'dark',
    brandTitle: 'qrtz ui',
    brandUrl: 'https://qrtz.ai',
    fontBase: "'Geist Pixel Square', ui-monospace, monospace",
    fontCode: "'Geist Pixel Square', ui-monospace, monospace",
    appBg: '#0a0a0a',
    appContentBg: '#0a0a0a',
    appBorderColor: '#262626',
    barBg: '#111111',
    colorPrimary: '#fafafa',
    colorSecondary: '#a3a3a3',
    textColor: '#fafafa',
  }),
});
