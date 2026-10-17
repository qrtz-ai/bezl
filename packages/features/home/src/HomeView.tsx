import { Text } from '@qrtz/ui';

export default function HomeView() {
  return (
    <div style={{ display: 'grid', gap: 8 }}>
      <Text as="h1" size="xl" weight={600}>
        qrtz
      </Text>
      <Text muted>Welcome. Pick a sub app from the sidebar.</Text>
    </div>
  );
}
