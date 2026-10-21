import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import {
  AppFrame,
  Button,
  Chip,
  Heading,
  IconButton,
  ListRow,
  MeetingRow,
  Page,
  SettingRow,
  SettingsSection,
  SIDENAV_WIDTH,
  SideNav,
  SplitView,
  Stack,
  Switch,
  Text,
  Toolbar,
} from '../src';
import { Frame, Row } from './_util';

const meta: Meta = { title: 'components/layout' };
export default meta;
type S = StoryObj;

function ShellDemo() {
  const [bar, setBar] = useState('nano');
  return (
    <Frame height={360}>
      <AppFrame
        embedded
        sidebar={
          <SideNav
            embedded
            label="apps"
            items={[
              { id: 'echo', label: 'echo', icon: 'mic' },
              { id: 'nano', label: 'nano', icon: 'edit-3' },
              { id: 'ctrl', label: 'ctrl', icon: 'command' },
              { id: 'snap', label: 'snap', icon: 'layout' },
              { id: 'apps', label: 'apps', group: 'foot' },
              {
                id: 'opts',
                label: 'opts',
                group: 'foot',
                dot: true,
                dotLabel: 'qrtz update ready',
              },
            ]}
            active={bar}
            onSelect={setBar}
          />
        }
      >
        <Page title={bar}>
          <Text muted>
            the {bar} page, in the shared page layout. the side nav is{' '}
            {SIDENAV_WIDTH}px wide.
          </Text>
        </Page>
      </AppFrame>
    </Frame>
  );
}
export const Shell: S = {
  name: 'app frame, side nav, page',
  render: () => <ShellDemo />,
};

export const PageStory: S = {
  name: 'page',
  render: () => (
    <Frame height={300}>
      <Page
        title="meetings"
        leading={<IconButton icon="back" label="back" onClick={() => {}} />}
        actions={
          <Button size="sm" icon="circle">
            record
          </Button>
        }
      >
        <Text>sections sit 24px apart.</Text>
        <Text muted>a second section.</Text>
      </Page>
    </Frame>
  ),
};

function SplitDemo() {
  const [sel, setSel] = useState(0);
  const meetings = ['weekly product sync', 'design review', '1:1 with grace'];
  return (
    <Frame height={300}>
      <SplitView
        sidebar={
          <>
            <Toolbar
              end={
                <IconButton icon="search" label="search" onClick={() => {}} />
              }
            >
              <Button size="sm" icon="circle">
                record
              </Button>
            </Toolbar>
            <Stack as="ul" gap={0} aria-label="meetings">
              {meetings.map((m, i) => (
                <MeetingRow
                  key={m}
                  title={m}
                  date={`oct ${12 + i}`}
                  selected={sel === i}
                  onSelect={() => setSel(i)}
                />
              ))}
            </Stack>
          </>
        }
      >
        <Heading level={2} preserveCase>
          {meetings[sel]}
        </Heading>
      </SplitView>
    </Frame>
  );
}
export const Split: S = {
  name: 'split view, toolbar, meeting row',
  render: () => <SplitDemo />,
};

function RowsDemo() {
  const [sel, setSel] = useState('a');
  const [chip, setChip] = useState(false);
  const [on, setOn] = useState(true);
  return (
    <>
      <Row label="list row">
        <div style={{ width: 360 }}>
          {['a', 'b'].map((id) => (
            <ListRow
              key={id}
              title={`note ${id}`}
              meta="oct 19"
              snippet="ship the beta friday"
              selected={sel === id}
              onSelect={() => setSel(id)}
            />
          ))}
        </div>
      </Row>
      <Row label="icon button and chip">
        <IconButton icon="search" label="search" onClick={() => {}} />
        <IconButton icon="stop" label="stop" tone="record" onClick={() => {}} />
        <Chip pressed={chip} onClick={() => setChip(!chip)}>
          grace
        </Chip>
      </Row>
      <Row label="opts section">
        <div style={{ width: '100%' }}>
          <SettingsSection title="recording">
            <SettingRow label="record automatically">
              <Switch
                checked={on}
                onChange={setOn}
                label="record automatically"
              />
            </SettingRow>
          </SettingsSection>
        </div>
      </Row>
      <Row label="stack">
        <Stack dir="row" gap={4} align="center">
          <Text>row</Text>
          <Text muted>gap 4</Text>
        </Stack>
      </Row>
    </>
  );
}
export const Rows: S = {
  name: 'list row, chip, opts rows, stack',
  render: () => <RowsDemo />,
};
