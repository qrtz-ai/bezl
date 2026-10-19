import {
  PILL_MOTION_DEFAULTS,
  PILL_MOTION_PARAMS,
  pillMotion,
  type PillMotion,
} from '@qrtz/core';
import { Button, Disclosure, SettingRow, Slider } from '@qrtz/ui';
import { useEffect, useState } from 'react';

/**
 * ADR 0221, TEMPORARY for tuning: the echo pill's springs, fades and corners as live sliders, collapsed at the
 * bottom of echo's page, in every build for now. Writes `talk.pillMotion` (always the whole object) when a drag
 * ends; the host forwards it to the helper, which uses it from the next expand on. reset: the snappy defaults.
 */
export function PillDevOpts({
  value,
  onCommit,
}: {
  value: unknown;
  onCommit: (m: PillMotion) => void;
}) {
  const stored = pillMotion(value);
  const [live, setLive] = useState<PillMotion>(stored);
  const key = JSON.stringify(stored);
  useEffect(() => setLive(pillMotion(JSON.parse(key))), [key]);
  return (
    <Disclosure title="dev opts" data-testid="talk-dev-opts">
      {PILL_MOTION_PARAMS.map((p) => (
        <SettingRow key={p.key} label={p.label}>
          <Slider
            label={p.label}
            unit={p.unit}
            min={p.min}
            max={p.max}
            step={p.step}
            value={live[p.key]}
            data-testid={`talk-motion-${p.key}`}
            onChange={(v) => setLive((m) => ({ ...m, [p.key]: v }))}
            onCommit={(v) => onCommit({ ...live, [p.key]: v })}
          />
        </SettingRow>
      ))}
      <SettingRow label="snappy defaults">
        <Button
          variant="secondary"
          data-testid="talk-motion-reset"
          onClick={() => {
            setLive(PILL_MOTION_DEFAULTS);
            onCommit(PILL_MOTION_DEFAULTS);
          }}
        >
          reset
        </Button>
      </SettingRow>
    </Disclosure>
  );
}
