import { Icon, type IconName } from '../../Icon';

/**
 * shadcn's generated components import lucide icons; qrtz has one icon set (Feather, via <Icon>), so the
 * generated files import these stand-ins instead. Size comes from <Icon size>, not shadcn's size-* classes.
 */
const make = (name: IconName) =>
  function ShadcnIcon(props: { className?: string }) {
    void props;
    return <Icon name={name} size="sm" />;
  };
export const CheckIcon = make('check');
export const ChevronDownIcon = make('chevron-down');
export const ChevronRightIcon = make('chevron-right');
export const ChevronUpIcon = make('chevron-up');
export const CircleIcon = make('circle');
export const XIcon = make('close');
