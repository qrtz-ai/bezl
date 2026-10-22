import type { ReactElement } from 'react';
import {
  ContextMenu as ContextMenuRoot,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from './components/ui/context-menu';
import { Icon } from './Icon';
import type { MenuItem } from './Menu';

export interface ContextMenuAction extends MenuItem {
  /** The item's data-testid. */
  testId?: string;
}

/**
 * A right-click menu on stock shadcn ContextMenu. The child (one focusable element) is the trigger: right-click,
 * the Menu key or Shift+F10 opens it at the pointer or the element; arrows move, Enter picks, Escape closes.
 * Labels are lowercase; danger items use shadcn's destructive variant.
 */
export function ContextMenu({
  items,
  label,
  children,
  'data-testid': testId,
}: {
  items: ContextMenuAction[];
  /** The menu's accessible name ("actions for this dictation"). */
  label: string;
  children: ReactElement;
  'data-testid'?: string;
}) {
  return (
    <ContextMenuRoot modal={false}>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent
        aria-label={label.toLowerCase()}
        data-q-no-ring
        data-testid={testId}
      >
        {items.map((it) => (
          <ContextMenuItem
            key={it.id}
            disabled={it.disabled}
            variant={it.danger ? 'destructive' : 'default'}
            data-testid={it.testId}
            onSelect={() => it.onSelect()}
          >
            {it.icon ? <Icon name={it.icon} size="sm" /> : null}
            {it.label.toLowerCase()}
          </ContextMenuItem>
        ))}
      </ContextMenuContent>
    </ContextMenuRoot>
  );
}
