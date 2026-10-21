import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  Badge,
  Button,
  Card,
  EmptyState,
  ErrorCallout,
  Field,
  Input,
  LongTextSample,
  PaneLoading,
  Progress,
  Select,
  Spinner,
  Switch,
  TextArea,
  userError,
} from '../src';
import { noop, Row, Width } from './_util';

/** The five states every view can be in (QRT-304): loading, error, empty, long text and disabled. */
const meta: Meta = { title: 'components/states' };
export default meta;
type S = StoryObj;

export const Loading: S = {
  render: () => (
    <>
      <Row label="buttons">
        <Button loading>saving</Button>
        <Button variant="primary" loading>
          connecting
        </Button>
      </Row>
      <Row label="spinner and progress">
        <Spinner label="loading" />
        <Width>
          <Progress value={0.4} label="downloading model" />
        </Width>
      </Row>
      <Row label="pane">
        <Width>
          <Card>
            <PaneLoading />
          </Card>
        </Width>
      </Row>
    </>
  ),
};

export const Error: S = {
  render: () => (
    <Width w={480}>
      <Row label="readable reason">
        <ErrorCallout
          error={userError(
            'the dictionary file is read-only.',
            'save the dictionary',
          )}
          action={<Button size="sm">try again</Button>}
        />
      </Row>
      <Row label="technical reason under details">
        <ErrorCallout
          error={userError('ECONNREFUSED 127.0.0.1:4317', 'reach the helper')}
          onDismiss={noop}
        />
      </Row>
      <Row label="badge">
        <Badge tone="danger">failed</Badge>
      </Row>
    </Width>
  ),
};

export const Empty: S = {
  render: () => (
    <Width w={480}>
      <Card>
        <EmptyState title="nothing dictated yet" />
      </Card>
      <Card>
        <EmptyState
          icon="grid"
          title="no agents yet"
          action={<Button>add an agent</Button>}
        />
      </Card>
    </Width>
  ),
};

export const LongTextState: S = {
  name: 'long text',
  render: () => <LongTextSample />,
};

export const Disabled: S = {
  render: () => (
    <Width>
      <Row label="buttons">
        <Button disabled>secondary</Button>
        <Button variant="primary" disabled>
          primary
        </Button>
        <Button variant="ghost" disabled>
          ghost
        </Button>
        <Button variant="danger" disabled>
          danger
        </Button>
      </Row>
      <Row label="fields">
        <Field label="name">
          <Input disabled value="echo" onChange={noop} />
        </Field>
        <Field label="model">
          <Select disabled value="small" onChange={noop}>
            <option value="small">small</option>
          </Select>
        </Field>
        <Field label="notes">
          <TextArea disabled value="read only" onChange={noop} />
        </Field>
      </Row>
      <Row label="switch">
        <Switch checked disabled label="on" onChange={noop} />
      </Row>
    </Width>
  ),
};
