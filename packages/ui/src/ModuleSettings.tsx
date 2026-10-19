import type { ReactNode } from 'react';
import { IconButton, Page } from './Layout';
import { Text } from './Text';

/**
 * ADR 0146 M1.4: a module's own settings page, shown inside its view (echo, nano, ctrl) in place of the view's
 * content. The global settings keep only what spans every module. `onClose` returns to the view.
 */
export function ModuleSettingsPage({
  module,
  onClose,
  children,
  inset,
  'data-testid': testId,
}: {
  /** Inside another page's column (nano's detail pane) rather than a page of its own. */
  inset?: boolean;
  module: string;
  onClose: () => void;
  children: ReactNode;
  'data-testid'?: string;
}) {
  return (
    <Page
      data-testid={testId}
      {...(inset ? { inset } : {})}
      title={`${module} opts`}
      actions={
        <IconButton
          icon="close"
          label={`close ${module} opts`}
          onClick={onClose}
          data-testid={testId ? `${testId}-close` : undefined}
        />
      }
    >
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
      label={`${module} opts`}
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
