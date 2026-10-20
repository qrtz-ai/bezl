import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import {
  Badge,
  Button,
  Callout,
  Card,
  CardGrid,
  Code,
  EmptyState,
  GridCard,
  Heading,
  Icon,
  Kbd,
  Label,
  Link,
  LongText,
  Mono,
  PaneLoading,
  Progress,
  Spinner,
  Text,
} from '../src';
import { Row, Width } from './_util';

const meta: Meta = { title: 'components/basics' };
export default meta;
type S = StoryObj;

export const Buttons: S = {
  name: 'button',
  render: () => (
    <>
      <Row label="variants">
        <Button variant="primary">primary</Button>
        <Button variant="secondary">secondary</Button>
        <Button variant="ghost">ghost</Button>
        <Button variant="danger">danger</Button>
      </Row>
      <Row label="sizes and icons">
        <Button size="sm">small</Button>
        <Button size="sm" icon="circle">
          record
        </Button>
        <Button icon="download">download</Button>
      </Row>
      <Row label="states">
        <Button loading>loading</Button>
        <Button disabled>disabled</Button>
      </Row>
      <Row label="hierarchy: one primary per view, secondary by default">
        <Button variant="primary">save</Button>
        <Button>export</Button>
        <Button variant="ghost">cancel</Button>
      </Row>
    </>
  ),
};

export const Badges: S = {
  name: 'badge',
  render: () => (
    <Row label="tones">
      {(['neutral', 'primary', 'success', 'warning', 'danger'] as const).map(
        (tone) => (
          <Badge key={tone} tone={tone}>
            {tone}
          </Badge>
        ),
      )}
    </Row>
  ),
};

export const Keys: S = {
  name: 'kbd',
  render: () => (
    <Row>
      <Kbd>⌘</Kbd>
      <Kbd>K</Kbd>
      <Kbd>ctrl</Kbd>
      <Kbd>fn</Kbd>
      <Kbd>esc</Kbd>
    </Row>
  ),
};

export const Typography: S = {
  name: 'text, heading, label, code, link',
  render: () => (
    <>
      <Row label="heading">
        <div>
          <Heading level={1}>heading 1</Heading>
          <Heading level={2}>heading 2</Heading>
          <Heading level={3}>heading 3</Heading>
        </div>
      </Row>
      <Row label="label">
        <Label>label</Label>
        <Label caps tone="muted">
          section label
        </Label>
      </Row>
      <Row label="text tones">
        {(['default', 'muted', 'success', 'warning', 'danger'] as const).map(
          (tone) => (
            <Text key={tone} tone={tone}>
              {tone}
            </Text>
          ),
        )}
      </Row>
      <Row label="code, mono, link">
        <Code>note.record()</Code>
        <Mono>00:12:34</Mono>
        <Link href="#">a link</Link>
        <Link onClick={() => {}}>a button link</Link>
      </Row>
      <Row label="code block">
        <Code block>bun run storybook</Code>
      </Row>
      <Row label="long text">
        <Width w={480}>
          <LongText>
            long text wraps at the column and keeps a comfortable measure, so a
            summary or a transcript paragraph never pushes the page sideways.
          </LongText>
        </Width>
      </Row>
    </>
  ),
};

export const Loading: S = {
  name: 'spinner, progress, pane loading',
  render: () => (
    <>
      <Row label="spinner">
        <Spinner />
        <Spinner size={18} label="loading" />
      </Row>
      <Row label="progress">
        {[0, 0.35, 0.6, 1].map((v) => (
          <Width key={v} w={160}>
            <Progress value={v} label={`progress ${v}`} />
          </Width>
        ))}
      </Row>
      <Row label="pane loading">
        <Width w={320}>
          <PaneLoading />
        </Width>
      </Row>
    </>
  ),
};

export const Callouts: S = {
  name: 'callout',
  render: () => (
    <Width w={560}>
      {(['info', 'warning', 'danger'] as const).map((tone) => (
        <div key={tone} style={{ marginBottom: 12 }}>
          <Callout
            tone={tone}
            title={tone}
            action={
              <Button size="sm" variant="secondary">
                action
              </Button>
            }
          />
        </div>
      ))}
    </Width>
  ),
};

function CardsDemo() {
  const [sel, setSel] = useState(false);
  return (
    <>
      <Row label="padding">
        <Card padding={3}>
          <Text>padding 3</Text>
        </Card>
        <Card padding={4}>
          <Text>padding 4</Text>
        </Card>
        <Card padding={6}>
          <Text>padding 6</Text>
        </Card>
      </Row>
      <Row label="interactive and selected">
        <Card interactive selected={sel} onClick={() => setSel(!sel)}>
          <Text>{sel ? 'selected' : 'click me'}</Text>
        </Card>
      </Row>
      <Row label="card grid">
        <div style={{ width: '100%' }}>
          <CardGrid>
            {['echo', 'nano', 'ctrl', 'snap'].map((n) => (
              <GridCard
                key={n}
                footer={
                  <Button size="sm" variant="secondary" block>
                    open
                  </Button>
                }
              >
                <Text>{n}</Text>
              </GridCard>
            ))}
          </CardGrid>
        </div>
      </Row>
    </>
  );
}
export const Cards: S = {
  name: 'card, card grid',
  render: () => <CardsDemo />,
};

export const Empty: S = {
  name: 'empty state',
  render: () => (
    <>
      <EmptyState
        icon="mic"
        title="no meetings"
        action={
          <Button size="sm" icon="circle">
            record
          </Button>
        }
      />
      <EmptyState icon="search" title="nothing found" />
    </>
  ),
};

export const Icons: S = {
  name: 'icon',
  render: () => (
    <Row label="tones">
      <Icon name="info" />
      <Icon name="info" tone="muted" />
      <Icon name="check" tone="success" />
      <Icon name="alert" tone="warning" />
      <Icon name="close" tone="danger" />
      <Icon name="circle" tone="record" fill />
    </Row>
  ),
};
