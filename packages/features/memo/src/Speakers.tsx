import { useState } from 'react';
import { Button, Input, Select, Text, useTheme } from '@qrtz/ui';
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
  const t = useTheme();
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
    <div style={{ display: 'grid', gap: 6 }} data-testid="speakers">
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: 6,
          alignItems: 'center',
        }}
      >
        <Text size="xs" muted>
          Speakers
        </Text>
        {speakers.map(([sp, n]) => (
          <button
            key={sp}
            type="button"
            data-testid="speaker-chip"
            aria-pressed={editing === sp}
            aria-label={`${sp} · ${n}: rename or merge`}
            onClick={() => {
              setEditing(editing === sp ? null : sp);
              setName(sp);
            }}
            style={{
              font: 'inherit',
              fontSize: 12,
              color: t.color.text,
              background: editing === sp ? t.color.accent : 'transparent',
              border: `1px solid ${t.color.border}`,
              borderRadius: 4,
              padding: '2px 8px',
              cursor: 'pointer',
            }}
          >
            {sp} <span style={{ color: t.color.textMuted }}>· {n}</span>
          </button>
        ))}
      </div>
      {editing ? (
        <form
          data-testid="speaker-editor"
          onSubmit={(e) => {
            e.preventDefault();
            apply(editing, name);
          }}
          style={{
            display: 'flex',
            gap: 8,
            alignItems: 'center',
            flexWrap: 'wrap',
          }}
        >
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            aria-label={`New name for ${editing}`}
            data-testid="speaker-name"
            maxLength={120}
            style={{ maxWidth: 220 }}
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
              style={{ maxWidth: 220 }}
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
        </form>
      ) : null}
      {error ? (
        <Text size="xs" style={{ color: t.color.danger }} role="alert">
          {error}
        </Text>
      ) : null}
    </div>
  );
}
