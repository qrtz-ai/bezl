import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  Button,
  ModuleSettingsButton,
  ModuleSettingsEmpty,
  ModuleSettingsPage,
  OnboardingFrame,
  PermissionPrompt,
  PermissionSetupList,
  PermissionSetupRow,
  SettingRow,
  Switch,
  Text,
} from '../src';
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
      <Frame height={280}>
        <ModuleSettingsPage module="echo" onClose={noop}>
          <SettingRow label="add a space after">
            <Switch checked onChange={noop} label="add a space after" />
          </SettingRow>
        </ModuleSettingsPage>
      </Frame>
      <Row label="empty">
        <ModuleSettingsEmpty>echo has no opts yet.</ModuleSettingsEmpty>
      </Row>
    </>
  ),
};
