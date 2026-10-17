import { useState } from 'react';
import { Button, Chip, Input, Select, Stack, Text } from '@qrtz/ui';
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
  const [error, setError] = useState<string | null>(null);
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
    setError(null);
    qrtz
      .invoke('memo:rename-speaker', { meetingId, from, to: target })
      .then(() => setEditing(null))
      .catch((e: Error) => setError(e.message));
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
            aria-label={`New name for ${editing}`}
            data-testid="speaker-name"
            maxLength={120}
          />
          <Button size="sm" type="submit" data-testid="speaker-save">
            Rename
          </Button>
          {speakers.length > 1 ? (
            <Select
              aria-label={`Merge ${editing} into another speaker`}
              data-testid="speaker-merge"
              value=""
              onChange={(e) => e.target.value && apply(editing, e.target.value)}
            >
              <option value="">Merge into…</option>
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
            Cancel
          </Button>
        </Stack>
      ) : null}
      {error ? (
        <Text variant="caption" tone="danger" role="alert">
          {error}
        </Text>
      ) : null}
    </Stack>
  );
}
