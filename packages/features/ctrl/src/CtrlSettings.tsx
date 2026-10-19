import { ModuleSettingsEmpty } from '@qrtz/ui';

/**
 * ADR 0146 M1.4: ctrl's own settings page, inside ctrl's view. ctrl keeps no settings yet (its tasks and schedules
 * are its data); the page is here so every module's settings live with the module.
 */
export default function CtrlSettings() {
  return (
    <ModuleSettingsEmpty>
      ctrl has no settings of its own yet. you can turn it off in marketplace →
      installed, where its accessibility and automation access are one click:
      allow, then switch on qrtz.
    </ModuleSettingsEmpty>
  );
}
