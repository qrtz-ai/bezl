import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import {
  Button,
  Dialog,
  Field,
  Input,
  ListItem,
  Menu,
  Modal,
  Select,
  SelectMenu,
  Switch,
  Tabs,
  KeepAlive,
  Text,
  TextArea,
  Tooltip,
  useToast,
} from '../src';
import { Row, Width } from './_util';

const meta: Meta = { title: 'components/forms and overlays' };
export default meta;
type S = StoryObj;

function InputsDemo() {
  const [on, setOn] = useState(true);
  return (
    <Width w={560}>
      <div style={{ display: 'grid', gap: 16 }}>
        <Field label="input">
          <Input placeholder="type here" />
        </Field>
        <Field label="disabled input">
          <Input placeholder="disabled" disabled />
        </Field>
        <Field label="select">
          <Select defaultValue="b">
            <option value="a">alpha</option>
            <option value="b">beta</option>
          </Select>
        </Field>
        <Field label="select menu (radix)">
          <SelectMenu
            value="dark"
            onValueChange={() => {}}
            options={[
              { value: 'dark', label: 'dark' },
              { value: 'light', label: 'light' },
            ]}
          />
        </Field>
        <Field label="textarea">
          <TextArea placeholder="notes" />
        </Field>
        <Field label="switch">
          <Switch
            checked={on}
            onChange={setOn}
            label={on ? 'on' : 'off'}
            showLabel
          />
        </Field>
      </div>
    </Width>
  );
}
export const Inputs: S = {
  name: 'field, input, select, textarea, switch',
  render: () => <InputsDemo />,
};

function TabsDemo() {
  const [tab, setTab] = useState<'summary' | 'notes' | 'transcript'>('summary');
  return (
    <>
      <Tabs
        tabs={[
          { id: 'summary', label: 'summary' },
          { id: 'notes', label: 'notes' },
          { id: 'transcript', label: 'transcript' },
        ]}
        value={tab}
        onChange={setTab}
      />
      {(['summary', 'notes', 'transcript'] as const).map((id) => (
        <KeepAlive key={id} active={tab === id}>
          <div style={{ paddingTop: 16 }}>
            <Text muted>{id} panel (kept alive while hidden)</Text>
          </div>
        </KeepAlive>
      ))}
    </>
  );
}
export const TabsStory: S = {
  name: 'tabs, keep alive',
  render: () => <TabsDemo />,
};

function OverlaysDemo() {
  const [dialog, setDialog] = useState(false);
  const [modal, setModal] = useState(false);
  const toast = useToast();
  return (
    <>
      <Row label="tooltip">
        <Tooltip label="start recording (⌘⇧r)">
          <Button variant="ghost" size="sm">
            hover me
          </Button>
        </Tooltip>
      </Row>
      <Row label="dialog, modal, toast">
        <Button variant="secondary" onClick={() => setDialog(true)}>
          open dialog
        </Button>
        <Button variant="secondary" onClick={() => setModal(true)}>
          open modal
        </Button>
        <Button
          variant="secondary"
          onClick={() => toast('saved to downloads', 'success')}
        >
          success toast
        </Button>
        <Button variant="secondary" onClick={() => toast('copied')}>
          info toast
        </Button>
        <Button
          variant="secondary"
          onClick={() => toast("couldn't save", 'error')}
        >
          error toast
        </Button>
      </Row>
      <Row label="menu">
        <Menu
          label="more actions for echo"
          items={[
            { id: 'opts', label: 'opts', icon: 'settings', onSelect: () => {} },
            {
              id: 'off',
              label: 'turn off',
              disabled: true,
              onSelect: () => {},
            },
            { id: 'rm', label: 'remove', danger: true, onSelect: () => {} },
          ]}
        />
        <Menu
          label="update waiting"
          badge
          items={[{ id: 'up', label: 'update', onSelect: () => {} }]}
        />
      </Row>
      <Row label="list item">
        <Width w={320}>
          <ListItem active>active item</ListItem>
          <ListItem onClick={() => {}} right={<Text muted>⌘1</Text>}>
            clickable item
          </ListItem>
        </Width>
      </Row>
      <Dialog
        open={dialog}
        onClose={() => setDialog(false)}
        title="delete meeting?"
      >
        <div
          style={{
            padding: 20,
            display: 'flex',
            gap: 8,
            justifyContent: 'flex-end',
          }}
        >
          <Button variant="ghost" onClick={() => setDialog(false)}>
            cancel
          </Button>
          <Button variant="danger" onClick={() => setDialog(false)}>
            delete
          </Button>
        </div>
      </Dialog>
      <Modal open={modal} onClose={() => setModal(false)} title="about qrtz">
        <Text>modals trap focus and close on esc.</Text>
      </Modal>
    </>
  );
}
export const Overlays: S = {
  name: 'tooltip, dialog, modal, toast, menu, list item',
  render: () => <OverlaysDemo />,
};

export const OpenModal: S = {
  name: 'modal (open)',
  render: () => (
    <Modal open onClose={() => {}} title="opts">
      <Text>an open modal, shown for review.</Text>
    </Modal>
  ),
};
