import { useState, type ReactNode } from 'react';
import {
  Badge,
  Button,
  Card,
  Dialog,
  EmptyState,
  Field,
  Heading,
  Input,
  Kbd,
  ListItem,
  Select,
  Spinner,
  Switch,
  Tabs,
  Text,
  TextArea,
  Tooltip,
  tokens,
  useTheme,
  useToast,
} from '@qrtz/ui';

function Section({
  id,
  title,
  children,
}: {
  id: string;
  title: string;
  children: ReactNode;
}) {
  const t = useTheme();
  return (
    <section
      data-testid={`gallery-${id}`}
      style={{ display: 'grid', gap: t.space[3] }}
    >
      <Text
        mono
        size="xs"
        muted
        style={{ textTransform: 'uppercase', letterSpacing: '0.08em' }}
      >
        {title}
      </Text>
      <Card padding={4}>
        <div
          style={{
            display: 'flex',
            flexWrap: 'wrap',
            gap: t.space[3],
            alignItems: 'center',
          }}
        >
          {children}
        </div>
      </Card>
    </section>
  );
}

export default function GalleryView() {
  const t = useTheme();
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [on, setOn] = useState(true);
  const [tab, setTab] = useState<'one' | 'two' | 'three'>('one');
  const [sel, setSel] = useState(1);
  return (
    <div style={{ display: 'grid', gap: t.space[8], maxWidth: 880 }}>
      <header style={{ display: 'grid', gap: t.space[2] }}>
        <Heading cursor>Gallery</Heading>
        <Text muted>Every @qrtz/ui component, in the qrtz.ai style.</Text>
      </header>

      <Section id="color" title="Color">
        {Object.entries(tokens.color)
          .filter(([k]) => k !== 'accentText')
          .map(([k, v]) => (
            <div key={k} style={{ display: 'grid', gap: 4, width: 88 }}>
              <div
                style={{
                  height: 40,
                  borderRadius: t.radius.md,
                  background: v,
                  border: `1px solid ${t.color.border}`,
                }}
              />
              <Text size="xs">{k}</Text>
              <Text size="xs" mono muted>
                {v}
              </Text>
            </div>
          ))}
      </Section>

      <Section id="type" title="Typography">
        <div style={{ display: 'grid', gap: t.space[2] }}>
          <Heading level={1}>Heading 1</Heading>
          <Heading level={2}>Heading 2</Heading>
          <Heading level={3} mono>
            Heading 3 mono
          </Heading>
          <Text>Body text: the live transcript is final.</Text>
          <Text muted size="sm">
            Muted small text
          </Text>
          <Text mono size="xs">
            mono xs · 00:12:34
          </Text>
        </div>
      </Section>

      <Section id="buttons" title="Buttons">
        <Button>Primary</Button>
        <Button variant="secondary">Secondary</Button>
        <Button variant="ghost">Ghost</Button>
        <Button variant="danger">Danger</Button>
        <Button size="sm">Small</Button>
        <Button loading>Loading</Button>
        <Button disabled>Disabled</Button>
      </Section>

      <Section id="badges" title="Badges, Kbd, Spinner, Tooltip">
        {(['neutral', 'primary', 'success', 'warning', 'danger'] as const).map(
          (tone) => (
            <Badge key={tone} tone={tone}>
              {tone}
            </Badge>
          ),
        )}
        <span>
          <Kbd>⌘</Kbd> <Kbd>K</Kbd>
        </span>
        <Spinner />
        <Tooltip label="Start recording (⌘⇧R)">
          <Button variant="ghost" size="sm">
            Hover me
          </Button>
        </Tooltip>
      </Section>

      <Section id="inputs" title="Inputs">
        <div
          style={{
            display: 'grid',
            gap: t.space[3],
            width: '100%',
            gridTemplateColumns: '1fr 1fr',
          }}
        >
          <Field label="Input" hint="With a hint">
            <Input placeholder="Type here…" />
          </Field>
          <Field label="Select">
            <Select defaultValue="b">
              <option value="a">Alpha</option>
              <option value="b">Beta</option>
            </Select>
          </Field>
          <Field label="TextArea">
            <TextArea placeholder="Notes…" />
          </Field>
          <Field label="Switch">
            <Switch checked={on} onChange={setOn} label={on ? 'On' : 'Off'} />
          </Field>
        </div>
      </Section>

      <Section id="nav" title="Tabs and list">
        <div style={{ display: 'grid', gap: t.space[3], width: '100%' }}>
          <Tabs
            tabs={[
              { id: 'one', label: 'Summary' },
              { id: 'two', label: 'Transcript' },
              { id: 'three', label: 'Action items' },
            ]}
            value={tab}
            onChange={setTab}
          />
          <div
            role="listbox"
            aria-label="Example meetings"
            style={{ display: 'grid', gap: 2, maxWidth: 360 }}
          >
            {['Weekly Product Sync', 'Design review', '1:1 with Grace'].map(
              (m, i) => (
                <ListItem
                  key={m}
                  active={sel === i}
                  onClick={() => setSel(i)}
                  right={
                    <Text size="xs" mono muted>
                      {12 + i * 7}:00
                    </Text>
                  }
                >
                  <Text size="sm">{m}</Text>
                </ListItem>
              ),
            )}
          </div>
        </div>
      </Section>

      <Section id="overlays" title="Dialog and toast">
        <Button
          variant="secondary"
          onClick={() => setOpen(true)}
          data-testid="gallery-open-dialog"
        >
          Open dialog
        </Button>
        <Button
          variant="secondary"
          onClick={() => toast('Saved to Downloads', 'success')}
          data-testid="gallery-toast"
        >
          Show toast
        </Button>
        <Dialog
          open={open}
          onClose={() => setOpen(false)}
          title="Delete meeting?"
        >
          <div
            style={{ padding: t.space[5], display: 'grid', gap: t.space[4] }}
          >
            <Text muted size="sm">
              This removes the transcript, summary and audio.
            </Text>
            <div
              style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}
            >
              <Button variant="ghost" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button variant="danger" onClick={() => setOpen(false)}>
                Delete
              </Button>
            </div>
          </div>
        </Dialog>
      </Section>

      <Section id="empty" title="Empty state">
        <div style={{ width: '100%' }}>
          <EmptyState
            icon="◎"
            title="No meetings yet"
            body="Join a Zoom or Meet call and qrtz will offer to record."
          />
        </div>
      </Section>
    </div>
  );
}
