import { useState, type KeyboardEvent, type MouseEvent } from 'react';
import * as DropdownMenuPrimitive from '@radix-ui/react-dropdown-menu';
import { useTheme } from './theme';
import { Icon, type IconName } from './Icon';
import { IconButton } from './Layout';
import { Text } from './Text';
import { popoverSurface, itemRow } from './primitives/surface';

export interface MenuItem {
  id: string;
  label: string;
  icon?: IconName;
  /** A destructive item (remove): drawn in the danger color. */
  danger?: boolean;
  disabled?: boolean;
  onSelect: () => void;
}

/**
 * An icon button that opens a small menu of actions (an installed app's "more actions"), built on Radix
 * DropdownMenu (shadcn `DropdownMenu`). Keyboard: Enter, Space or ArrowDown open it on the first item, ArrowUp on
 * the last; ArrowUp/ArrowDown/Home/End move; Enter or Space picks; Escape closes and returns focus to the button;
 * Tab or a click outside closes. Clicks and keys inside never reach a clickable row around it. Item ids become
 * data-testids `<data-testid>-<id>`. Rendered inline (no Portal), non-modal, so the page stays interactive.
 */
export function Menu({
  label,
  icon = 'more-horizontal',
  items,
  badge,
  'data-testid': testId,
}: {
  /** The button's accessible name ("more actions for echo"). */
  label: string;
  icon?: IconName;
  items: MenuItem[];
  /** A dot on the button (an update is waiting). */
  badge?: boolean;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const [open, setOpen] = useState(false);
  const enabled = items.filter((i) => !i.disabled);
  const stop = (e: MouseEvent | KeyboardEvent) => e.stopPropagation();
  return (
    <span
      onClick={stop}
      onKeyDown={stop}
      style={{ position: 'relative', display: 'inline-flex' }}
    >
      <DropdownMenuPrimitive.Root
        modal={false}
        open={open}
        onOpenChange={setOpen}
      >
        <DropdownMenuPrimitive.Trigger asChild disabled={!enabled.length}>
          <IconButton
            icon={icon}
            label={label}
            badge={badge}
            disabled={!enabled.length}
            data-testid={testId}
            // Radix opens on pointerdown and keydown; a synthetic click (assistive tech, el.click()) has no pointer.
            onClick={(e) => {
              if (e.detail === 0) setOpen((o) => !o);
            }}
          />
        </DropdownMenuPrimitive.Trigger>
        <DropdownMenuPrimitive.Content
          align="end"
          sideOffset={4}
          loop
          aria-label={label.toLowerCase()}
          data-q-no-ring
          data-testid={testId ? `${testId}-menu` : undefined}
          onCloseAutoFocus={(e) => e.stopPropagation()}
          style={popoverSurface(t)}
        >
          {items.map((it) => (
            <DropdownMenuPrimitive.Item
              key={it.id}
              disabled={it.disabled}
              data-q-item=""
              data-testid={testId ? `${testId}-${it.id}` : undefined}
              data-danger={it.danger ? '' : undefined}
              onClick={(e) => e.stopPropagation()}
              onSelect={() => it.onSelect()}
              style={{
                ...itemRow(t, it.disabled),
                color: it.danger ? t.color.danger : t.color.text,
              }}
            >
              {it.icon ? <Icon name={it.icon} size="sm" /> : null}
              <Text variant="bodySm" tone={it.danger ? 'danger' : 'default'}>
                {it.label.toLowerCase()}
              </Text>
            </DropdownMenuPrimitive.Item>
          ))}
        </DropdownMenuPrimitive.Content>
      </DropdownMenuPrimitive.Root>
    </span>
  );
}
