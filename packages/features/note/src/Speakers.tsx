import { useState } from 'react';
import {
  Button,
  Chip,
  ErrorCallout,
  Input,
  Select,
  Stack,
  useUserError,
} from '@qrtz/ui';
import type { Segment } from '@qrtz/core';
import { useQrtz } from '@qrtz/core/react';

/** Speaker chips with rename and merge (merge = rename onto another speaker's name). */
export function Speakers({
  meetingId,
  segments,
}: {
  meetingId: string;
  segments: Segment[];
}) {
  const qrtz = useQrtz();
  const [editing, setEditing] = useState<string | null>(null);
  const [name, setName] = useState('');
  const { error, fail, clear } = useUserError();
  const counts = new Map<string, number>();
  for (const s of segments) {
    const k = s.speakerName ?? s.speaker;
    if (k) counts.set(k, (counts.get(k) ?? 0) + 1);
  }
  const speakers = [...counts.entries()].sort((a, b) => b[1] - a[1]);
  if (!speakers.length) return null;

  const apply = (from: string, to: string) => {
    const target = to.trim();
    if (!target || target === from) return setEditing(null);
    clear();
    qrtz
      .invoke('note:rename-speaker', { meetingId, from, to: target })
      .then(() => setEditing(null))
      .catch(fail('rename the speaker'));
  };

  return (
    <Stack gap={2} data-testid="speakers">
      <Stack dir="row" gap={1} wrap align="center">
        {speakers.map(([sp, n]) => (
          <Chip
            key={sp}
            data-testid="speaker-chip"
            pressed={editing === sp}
            aria-label={`${sp} · ${n}: rename or merge`}
            onClick={() => {
              setEditing(editing === sp ? null : sp);
              setName(sp);
            }}
          >
            {sp}
          </Chip>
        ))}
      </Stack>
      {editing ? (
        <Stack
          as="form"
          dir="row"
          gap={2}
          align="center"
          wrap
          data-testid="speaker-editor"
          onSubmit={() => apply(editing, name)}
        >
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            aria-label={`new name for ${editing}`}
            data-testid="speaker-name"
            maxLength={120}
          />
          <Button size="sm" type="submit" data-testid="speaker-save">
            rename
          </Button>
          {speakers.length > 1 ? (
            <Select
              aria-label={`merge ${editing} into another speaker`}
              data-testid="speaker-merge"
              value=""
              onChange={(e) => e.target.value && apply(editing, e.target.value)}
            >
              <option value="">merge into…</option>
              {speakers
                .filter(([sp]) => sp !== editing)
                .map(([sp]) => (
                  <option key={sp} value={sp}>
                    {sp}
                  </option>
                ))}
            </Select>
          ) : null}
          <Button
            size="sm"
            variant="ghost"
            type="button"
            onClick={() => setEditing(null)}
          >
            cancel
          </Button>
        </Stack>
      ) : null}
      <ErrorCallout
        error={error}
        onDismiss={clear}
        data-testid="speaker-error"
      />
    </Stack>
  );
}
