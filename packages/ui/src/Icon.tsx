import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  Circle,
  Clipboard,
  Clock,
  Command,
  Copy,
  Download,
  Edit2,
  Grid,
  Info,
  MessageCircle,
  Mic,
  MicOff,
  MoreHorizontal,
  Pause,
  Play,
  RefreshCw,
  Search,
  Settings,
  Square,
  Trash2,
  VolumeX,
  X,
  type Icon as FeatherIcon,
} from 'react-feather';
import { useTheme } from './theme';
import type { Tokens } from './tokens';

/**
 * The one icon set: Feather (react-feather, MIT). Every glyph in the app goes through <Icon name>; feature
 * code never imports react-feather (scripts/check-ui-purity.mjs enforces it). Add icons here, not inline.
 */
export const ICONS = {
  alert: AlertTriangle,
  check: Check,
  'chevron-down': ChevronDown,
  'chevron-right': ChevronRight,
  circle: Circle,
  clipboard: Clipboard,
  clock: Clock,
  command: Command,
  close: X,
  copy: Copy,
  download: Download,
  edit: Edit2,
  grid: Grid,
  info: Info,
  ask: MessageCircle,
  mic: Mic,
  'mic-off': MicOff,
  more: MoreHorizontal,
  pause: Pause,
  play: Play,
  refresh: RefreshCw,
  search: Search,
  settings: Settings,
  stop: Square,
  todo: Square,
  trash: Trash2,
  'no-audio': VolumeX,
} satisfies Record<string, FeatherIcon>;

export type IconName = keyof typeof ICONS;
export type IconSize = keyof Tokens['icon']['size'];
export const ICON_NAMES = Object.keys(ICONS) as IconName[];

export function isIconName(v: unknown): v is IconName {
  return typeof v === 'string' && v in ICONS;
}

/**
 * A Feather icon at a token size. Decorative (aria-hidden) unless `label` is given. `fill` fills the
 * shape with the current color (record dot, stop square). Color follows the surrounding text color
 * unless a tone is given.
 */
export function Icon({
  name,
  size = 'md',
  label,
  fill,
  tone,
  'data-testid': testId,
}: {
  name: IconName;
  size?: IconSize;
  label?: string;
  fill?: boolean;
  tone?: 'muted' | 'danger' | 'warning' | 'success' | 'record';
  'data-testid'?: string;
}) {
  const t = useTheme();
  const Glyph = ICONS[name];
  const color =
    tone === 'muted'
      ? t.color.textMuted
      : tone === 'record'
        ? t.color.recording
        : tone
          ? t.color[tone]
          : 'currentColor';
  return (
    <Glyph
      size={t.icon.size[size]}
      strokeWidth={t.icon.stroke}
      color={color}
      fill={fill ? color : 'none'}
      aria-hidden={label ? undefined : true}
      aria-label={label}
      role={label ? 'img' : undefined}
      focusable="false"
      data-testid={testId}
      data-icon={name}
      style={{ flexShrink: 0, display: 'block' }}
    />
  );
}
