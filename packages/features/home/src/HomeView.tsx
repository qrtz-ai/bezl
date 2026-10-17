import { Badge, Heading, Kbd, Text } from '@qrtz/ui';

export default function HomeView() {
  return (
    <div style={{ display: 'grid', gap: 16, maxWidth: 640 }}>
      <Heading cursor style={{ fontSize: 48 }}>
        qrtz
      </Heading>
      <Text muted size="lg">
        The intelligence layer, on your desktop.
      </Text>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <Badge>alpha</Badge>
        <Text muted size="sm">
          Pick a sub app from the sidebar · <Kbd>⌘</Kbd> <Kbd>K</Kbd> coming
          soon
        </Text>
      </div>
    </div>
  );
}
