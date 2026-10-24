import { Button, ModuleSettingsEmpty, Stack } from '@qrtz/ui';
import { requestNavigate } from '@qrtz/core';

/**
 * ADR 0146 M1.4: ctrl's own settings page, inside ctrl's view. ctrl keeps no settings yet (its history
 * is its data); the page is here so every module's settings live with the module. Switching it off and its
 * permissions are in its detail on the qrtz page.
 */
export default function CtrlSettings() {
  return (
    <Stack gap={3} align="start">
      <ModuleSettingsEmpty>
        ctrl has no opts of its own yet. you can turn it off in qrtz, where its
        accessibility access is one click: allow, then switch on qrtz.
      </ModuleSettingsEmpty>
      <Button
        size="sm"
        variant="secondary"
        icon="open"
        onClick={() => requestNavigate({ app: 'qrtz', item: 'ctrl' })}
        data-testid="ctrl-open-in-qrtz"
      >
        ctrl in bezl
      </Button>
    </Stack>
  );
}
