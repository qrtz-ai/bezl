import type { ReactNode } from 'react';
import { Modal } from './Modal';
import { Text } from './Text';
import { useTheme } from './theme';

/**
 * ADR 0146 M1.4: a module's own opts, in a modal over its view (echo, nano, ctrl, snap), so the view stays put
 * underneath. Esc, the scrim and the close button all call `onClose`; focus is trapped inside while it's open.
 */
export function ModuleSettingsModal({
  module,
  open,
  onClose,
  children,
  width = 640,
  'data-testid': testId,
}: {
  module: string;
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  width?: number;
  'data-testid'?: string;
}) {
  const t = useTheme();
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`${module} opts`}
      closeLabel={`close ${module} opts`}
      width={width}
      initialFocus="dialog"
      {...(testId ? { 'data-testid': testId } : {})}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: t.space[6],
          minWidth: 0,
        }}
      >
        {children}
      </div>
    </Modal>
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
