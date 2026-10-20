import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  Button,
  ModuleSettingsButton,
  ModuleSettingsEmpty,
  ModuleSettingsModal,
  OnboardingFrame,
  PermissionPrompt,
  PermissionSetupList,
  PermissionSetupRow,
  SettingRow,
  SettingsSection,
  Disclosure,
  Field,
  Input,
  Switch,
  Text,
} from '../src';
import { useState } from 'react';
import { Frame, Row, Width, noop } from './_util';

const meta: Meta = { title: 'components/setup and opts' };
export default meta;
type S = StoryObj;

export const Onboarding: S = {
  name: 'onboarding, permission setup',
  render: () => (
    <Frame height={520}>
      <OnboardingFrame
        title="welcome to qrtz"
        actions={
          <Button variant="ghost" size="sm">
            skip
          </Button>
        }
      >
        <PermissionSetupList aria-label="permissions">
          <PermissionSetupRow
            id="mic"
            icon="mic"
            label="microphone"
            reason="echo hears you"
            state="granted"
          />
          <PermissionSetupRow
            id="ax"
            icon="accessibility"
            label="accessibility"
            reason="echo types for you"
            state="not-granted"
            onAllow={noop}
            onOpenSettings={noop}
          />
          <PermissionSetupRow
            id="scr"
            icon="screen-recording"
            label="screen recording"
            reason="nano hears meetings"
            state="needs-restart"
            onRestart={noop}
          />
          <PermissionSetupRow
            id="cal"
            icon="calendar"
            label="calendar"
            reason="nano knows your meetings"
            state="checking"
          />
          <PermissionSetupRow
            id="cam"
            icon="grid"
            label="camera"
            reason="nothing uses it"
            state="not-needed"
          />
        </PermissionSetupList>
      </OnboardingFrame>
    </Frame>
  ),
};

export const Prompt: S = {
  name: 'permission prompt',
  render: () => (
    <>
      <Row label="ask">
        <Width w={480}>
          <PermissionPrompt
            title="echo needs accessibility"
            pane="accessibility"
            onOpen={noop}
          >
            <Text muted>so it can type what you say.</Text>
          </PermissionPrompt>
        </Width>
      </Row>
      <Row label="waiting">
        <Width w={480}>
          <PermissionPrompt
            title="echo needs accessibility"
            pane="accessibility"
            onOpen={noop}
            waiting
            onRestart={noop}
          />
        </Width>
      </Row>
    </>
  ),
};

export const ModuleOpts: S = {
  name: 'module opts',
  render: () => (
    <>
      <Row label="button">
        <ModuleSettingsButton module="echo" onClick={noop} />
      </Row>
      <Frame height={360}>
        <ModuleSettingsModal module="echo" open onClose={noop}>
          <SettingRow label="add a space after">
            <Switch checked onChange={noop} label="add a space after" />
          </SettingRow>
        </ModuleSettingsModal>
      </Frame>
      <Row label="empty">
        <ModuleSettingsEmpty>echo has no opts yet.</ModuleSettingsEmpty>
      </Row>
    </>
  ),
};

function OptsModalDemo() {
  const [open, setOpen] = useState(true);
  const [space, setSpace] = useState(true);
  return (
    <Frame height={360}>
      <div style={{ padding: 24 }}>
        <ModuleSettingsButton module="echo" onClick={() => setOpen(true)} />
      </div>
      <ModuleSettingsModal
        module="echo"
        open={open}
        onClose={() => setOpen(false)}
      >
        <SettingsSection title="dictation" description="how echo listens.">
          <SettingRow label="add a space after">
            <Switch
              checked={space}
              onChange={setSpace}
              label="add a space after"
            />
          </SettingRow>
          <SettingRow
            label="warm mic"
            description="keeps the mic open for a faster start."
          >
            <Switch checked={false} onChange={noop} label="warm mic" />
          </SettingRow>
          <Field label="toggle shortcut">
            <Input defaultValue="CommandOrControl+Shift+Space" />
          </Field>
        </SettingsSection>
        <SettingsSection title="text">
          <SettingRow label="clean up text">
            <Switch checked onChange={noop} label="clean up text" />
          </SettingRow>
        </SettingsSection>
        <Disclosure title="dev opts" defaultOpen>
          <SettingRow label="snappy defaults">
            <Switch checked onChange={noop} label="snappy defaults" />
          </SettingRow>
        </Disclosure>
      </ModuleSettingsModal>
    </Frame>
  );
}

/** Click the gear: the opts modal opens over the view; esc, the scrim or the close button closes it. */
export const ModuleOptsModalDark: S = {
  name: 'module opts modal, dark',
  globals: { theme: 'dark' },
  render: () => <OptsModalDemo />,
};

export const ModuleOptsModalLight: S = {
  name: 'module opts modal, light',
  globals: { theme: 'light' },
  render: () => <OptsModalDemo />,
};
