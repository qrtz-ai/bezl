import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { Disclosure, SettingRow, Slider, ThemeScope } from '../src';
import { Width } from './_util';

const meta: Meta = { title: 'components/slider' };
export default meta;
type S = StoryObj;

function Demo() {
  const [d, setD] = useState(0.3);
  const [b, setB] = useState(0.15);
  const [r, setR] = useState(14);
  return (
    <div style={{ display: 'grid', gap: 12 }}>
      <SettingRow label="expand duration">
        <Slider
          label="expand duration"
          unit="s"
          min={0.05}
          max={1.5}
          value={d}
          onChange={setD}
        />
      </SettingRow>
      <SettingRow label="expand bounce">
        <Slider
          label="expand bounce"
          min={0}
          max={0.35}
          value={b}
          onChange={setB}
        />
      </SettingRow>
      <SettingRow label="radius expanded">
        <Slider
          label="radius expanded"
          unit="pt"
          min={0}
          max={32}
          step={0.5}
          value={r}
          onChange={setR}
        />
      </SettingRow>
      <Disclosure title="dev opts">
        <Slider label="disclosed" min={0} max={1} value={0.5} />
      </Disclosure>
    </div>
  );
}

export const LightAndDark: S = {
  name: 'slider and disclosure, light and dark',
  render: () => (
    <div style={{ display: 'grid', gap: 16 }}>
      {(['light', 'dark'] as const).map((m) => (
        <ThemeScope key={m} mode={m} style={{ padding: 16 }}>
          <Width w={480}>
            <Demo />
          </Width>
        </ThemeScope>
      ))}
    </div>
  ),
};
