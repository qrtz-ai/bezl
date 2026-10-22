import {
  AlertTriangle,
  ArrowUpCircle,
  Video,
  ChevronLeft,
  Cpu,
  Database,
  Folder,
  Monitor,
  MousePointer,
  Package,
  Zap,
  Calendar,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  Circle,
  Clipboard,
  Clock,
  Command,
  Copy,
  Download,
  ExternalLink,
  Edit2,
  Edit3,
  Grid,
  Info,
  Layout,
  Lock,
  MessageCircle,
  Mic,
  MicOff,
  MoreHorizontal,
  Pause,
  Play,
  RefreshCw,
  RotateCcw,
  RotateCw,
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
  // ADR 0161: the marketplace and module manager.
  back: ChevronLeft,
  update: ArrowUpCircle,
  package: Package,
  service: Cpu,
  model: Database,
  folder: Folder,
  'screen-recording': Monitor,
  accessibility: MousePointer,
  automation: Zap,
  check: Check,
  // ADR 0221: the notch surface's built-in set.
  calendar: Calendar,
  'chevron-down': ChevronDown,
  'chevron-right': ChevronRight,
  'chevron-up': ChevronUp,
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
  // snap's glyph: a window split into panes.
  layout: Layout,
  ask: MessageCircle,
  mic: Mic,
  // nano's glyph: a pen writing a line.
  'edit-3': Edit3,
  'mic-off': MicOff,
  more: MoreHorizontal,
  // The qrtz page's per-app actions menu.
  'more-horizontal': MoreHorizontal,
  pause: Pause,
  play: Play,
  refresh: RefreshCw,
  undo: RotateCcw,
  search: Search,
  settings: Settings,
  stop: Square,
  todo: Square,
  trash: Trash2,
  'no-audio': VolumeX,
  // Permission prompts: open the settings pane, the permission itself, and "restart qrtz".
  open: ExternalLink,
  permission: Lock,
  restart: RotateCw,
  // ADR 0217: an upcoming event's video-call link.
  video: Video,
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
