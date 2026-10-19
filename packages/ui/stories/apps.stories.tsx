import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import {
  AppList,
  AppRow,
  Button,
  ExtensionCard,
  ExtensionGlyph,
  ExtensionHeader,
  ExtensionStatusBadge,
  FactList,
  IconLine,
  IconLines,
  InstallProgress,
  ModuleRow,
  RowProgress,
  type ExtensionStatus,
} from '../src';
import { Row, Width, noop } from './_util';

const meta: Meta = { title: 'components/apps' };
export default meta;
type S = StoryObj;
const STATUSES: ExtensionStatus[] = [
  'available',
  'installed',
  'update',
  'built-in',
  'working',
];

export const Status: S = {
  name: 'status badge, glyph',
  render: () => (
    <>
      <Row label="status">
        {STATUSES.map((s) => (
          <ExtensionStatusBadge key={s} status={s} />
        ))}
      </Row>
      <Row label="glyph sizes">
        {(['sm', 'md', 'lg'] as const).map((s) => (
          <ExtensionGlyph key={s} icon="mic" size={s} />
        ))}
      </Row>
    </>
  ),
};

export const Cards: S = {
  name: 'app card',
  render: () => (
    <Row>
      {STATUSES.map((s) => (
        <Width key={s} w={240}>
          <ExtensionCard
            name="nano"
            icon="edit-3"
            description="meeting notes, on your mac"
            meta="v0.3.0 · 628 mb with dependencies"
            status={s}
            progress={s === 'working' ? 0.4 : null}
            action={
              <Button size="sm" variant="secondary" block>
                {s === 'available'
                  ? 'install'
                  : s === 'update'
                    ? 'update'
                    : 'open'}
              </Button>
            }
            onOpen={noop}
          />
        </Width>
      ))}
    </Row>
  ),
};

export const Detail: S = {
  name: 'app detail',
  render: () => (
    <div style={{ display: 'grid', gap: 24 }}>
      <ExtensionHeader
        name="echo"
        icon="mic"
        description="dictation that stays on your mac"
        status="installed"
        action={<Button size="sm">open</Button>}
      />
      <FactList
        items={[
          { label: 'version', value: '0.2.3' },
          { label: 'size', value: '412 mb' },
          { label: 'runs', value: 'on-device' },
        ]}
      />
      <IconLines aria-label="what it can use">
        <IconLine icon="mic" title="microphone" meta="while you hold fn" />
        <IconLine icon="permission" title="no network" />
      </IconLines>
      <Width w={360}>
        <InstallProgress
          value={0.42}
          label="installing echo"
          detail="173 of 412 mb"
        />
      </Width>
    </div>
  ),
};

function ListDemo() {
  const [on, setOn] = useState(true);
  return (
    <>
      <AppList aria-label="apps">
        <AppRow
          name="echo"
          icon="mic"
          description="dictation"
          meta="v0.2.0 · built in"
          status="built in"
          onOpen={noop}
        />
        <AppRow
          name="nano"
          icon="edit-3"
          description="notetaker"
          meta="v0.3.0"
          status="update"
          statusTone="warning"
          action={
            <Button size="sm" variant="secondary">
              update
            </Button>
          }
          onOpen={noop}
        />
        <AppRow
          name="snap"
          icon="grid"
          description="screenshots"
          meta="v0.1.0"
          status="couldn't install"
          statusTone="danger"
          onOpen={noop}
        />
        <AppRow
          name="ctrl"
          icon="command"
          description="ctrl do"
          meta="v0.1.0"
          status="installed"
          statusTone="success"
          onOpen={noop}
        />
      </AppList>
      <Row label="row progress">
        <Width w={240}>
          <RowProgress value={0.6} label="downloading nano" />
        </Width>
      </Row>
      <Row label="module row">
        <div style={{ width: '100%' }}>
          <ModuleRow
            name="nano"
            icon="edit-3"
            meta="v0.3.0"
            status="installed"
            enabled={on}
            onToggle={setOn}
          />
          <ModuleRow
            name="echo"
            icon="mic"
            meta="v0.2.0"
            status="built-in"
            enabled
            toggleDisabled
            onToggle={noop}
          />
        </div>
      </Row>
    </>
  );
}
export const List: S = {
  name: 'app list, module row',
  render: () => <ListDemo />,
};
