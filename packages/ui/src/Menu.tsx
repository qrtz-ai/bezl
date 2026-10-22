import { useState, type KeyboardEvent, type MouseEvent } from 'react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from './components/ui/dropdown-menu';
import { Icon, type IconName } from './Icon';
import { IconButton } from './Layout';

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
 * An icon button that opens a small menu of actions (an installed app's "more actions"), on shadcn's DropdownMenu.
 * Keyboard: Enter, Space or ArrowDown open it on the first item, ArrowUp on the last; ArrowUp/ArrowDown/Home/End
 * move; Enter or Space picks; Escape closes and returns focus to the button; Tab or a click outside closes. Clicks
 * and keys inside never reach a clickable row around it. Item ids become data-testids `<data-testid>-<id>`.
 * Inline (no Portal) and non-modal, so the page stays interactive and a hidden sub app's menu stays hidden.
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
  const [open, setOpen] = useState(false);
  const enabled = items.filter((i) => !i.disabled);
  const stop = (e: MouseEvent | KeyboardEvent) => e.stopPropagation();
  return (
    <span
      onClick={stop}
      onKeyDown={stop}
      className="tw:relative tw:inline-flex"
    >
      <DropdownMenu modal={false} open={open} onOpenChange={setOpen}>
        <DropdownMenuTrigger asChild disabled={!enabled.length}>
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
        </DropdownMenuTrigger>
        <DropdownMenuContent
          inline
          align="end"
          loop
          aria-label={label.toLowerCase()}
          data-q-no-ring
          data-testid={testId ? `${testId}-menu` : undefined}
          onCloseAutoFocus={(e) => e.stopPropagation()}
        >
          {items.map((it) => (
            <DropdownMenuItem
              key={it.id}
              disabled={it.disabled}
              variant={it.danger ? 'destructive' : 'default'}
              data-testid={testId ? `${testId}-${it.id}` : undefined}
              data-danger={it.danger ? '' : undefined}
              onClick={(e) => e.stopPropagation()}
              onSelect={() => it.onSelect()}
            >
              {it.icon ? <Icon name={it.icon} size="sm" /> : null}
              {it.label.toLowerCase()}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  );
}
