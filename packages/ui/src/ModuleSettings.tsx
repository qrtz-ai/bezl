import type { ReactNode } from 'react';
import { Heading } from './Heading';
import { IconButton, Page, Stack } from './Layout';
import { Text } from './Text';

/**
 * ADR 0146 M1.4: a module's own settings page, shown inside its view (echo, nano, ctrl) in place of the view's
 * content. The global settings keep only what spans every module. `onClose` returns to the view.
 */
export function ModuleSettingsPage({
  module,
  onClose,
  children,
  'data-testid': testId,
}: {
  module: string;
  onClose: () => void;
  children: ReactNode;
  'data-testid'?: string;
}) {
  return (
    <Page data-testid={testId}>
      <Stack dir="row" align="center" justify="between">
        <Heading>{`${module} settings`}</Heading>
        <IconButton
          icon="close"
          label={`close ${module} settings`}
          onClick={onClose}
          data-testid={testId ? `${testId}-close` : undefined}
        />
      </Stack>
      {children}
    </Page>
  );
}

/** The gear that opens a module's settings page from its view. */
export function ModuleSettingsButton({
  module,
  onClick,
  'data-testid': testId,
}: {
  module: string;
  onClick: () => void;
  'data-testid'?: string;
}) {
  return (
    <IconButton
      icon="settings"
      label={`${module} settings`}
      onClick={onClick}
      data-testid={testId}
    />
  );
}

/** A module with nothing to set yet says so, instead of an empty page. */
export function ModuleSettingsEmpty({ children }: { children: ReactNode }) {
  return (
    <Text variant="caption" muted data-testid="module-settings-empty">
      {children}
    </Text>
  );
}
