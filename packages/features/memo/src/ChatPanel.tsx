import { useRef, useState } from 'react';
import {
  Button,
  Callout,
  Card,
  Input,
  Spinner,
  Text,
  useTheme,
} from '@qrtz/ui';
import type { ChatAnswer } from '@qrtz/core';
import { useQrtz } from '@qrtz/core/react';
import { fmtClock } from './format';

type Turn =
  | { role: 'user'; content: string }
  | { role: 'assistant'; content: string; answer: ChatAnswer };

/**
 * Grounded Q&A. With `meetingId`, asks about that meeting; without, asks across all meetings (FTS retrieval).
 * Answers cite transcript lines; clicking a source opens the meeting at that moment.
 */
export function ChatPanel({
  meetingId,
  onOpenSource,
}: {
  meetingId?: string;
  onOpenSource?: (meetingId: string, startMs: number) => void;
}) {
  const qrtz = useQrtz();
  const t = useTheme();
  const [turns, setTurns] = useState<Turn[]>([]);
  const [q, setQ] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const endRef = useRef<HTMLDivElement>(null);

  const ask = async (question: string, prior: Turn[]) => {
    setBusy(true);
    setError(null);
    setTurns([...prior, { role: 'user', content: question }]);
    try {
      const answer = await qrtz.invoke('memo:chat', {
        question,
        ...(meetingId ? { meetingId } : {}),
        history: prior.map((x) => ({ role: x.role, content: x.content })),
      });
      setTurns([
        ...prior,
        { role: 'user', content: question },
        { role: 'assistant', content: answer.answer, answer },
      ]);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
      requestAnimationFrame(() =>
        endRef.current?.scrollIntoView({ block: 'end' }),
      );
    }
  };

  const submit = () => {
    const question = q.trim();
    if (!question || busy) return;
    setQ('');
    void ask(question, turns);
  };
  const lastUser = [...turns].reverse().find((x) => x.role === 'user');

  return (
    <div style={{ display: 'grid', gap: 12 }} data-testid="chat">
      {turns.length === 0 ? (
        <Text muted size="sm">
          {meetingId
            ? 'Ask anything about this meeting, e.g. "What did we decide about the launch?"'
            : 'Ask across all your meetings, e.g. "When did we last talk about pricing?"'}
        </Text>
      ) : null}
      <div role="log" aria-live="polite" style={{ display: 'grid', gap: 10 }}>
        {turns.map((x, i) =>
          x.role === 'user' ? (
            <Text key={i} weight={600} data-testid="chat-question">
              {x.content}
            </Text>
          ) : (
            <Card key={i} padding={3} data-testid="chat-answer">
              {x.answer.llmError ? (
                <div style={{ marginBottom: 8 }}>
                  <Callout
                    tone="warning"
                    title="AI answer failed. Showing matching quotes instead."
                    data-testid="chat-llm-error"
                  >
                    {x.answer.llmError}
                  </Callout>
                </div>
              ) : null}
              <Text as="p" style={{ whiteSpace: 'pre-wrap', lineHeight: 1.55 }}>
                {x.content}
              </Text>
              {x.answer.sources.length ? (
                <div
                  style={{
                    display: 'flex',
                    flexWrap: 'wrap',
                    gap: 6,
                    marginTop: 8,
                  }}
                >
                  {x.answer.sources.slice(0, 6).map((s, j) => (
                    <button
                      key={j}
                      type="button"
                      data-testid="chat-source"
                      onClick={() => onOpenSource?.(s.meetingId, s.startMs)}
                      title={s.text}
                      style={{
                        font: 'inherit',
                        fontSize: 12,
                        color: t.color.textMuted,
                        background: 'transparent',
                        border: `1px solid ${t.color.border}`,
                        borderRadius: 4,
                        padding: '2px 6px',
                        cursor: 'pointer',
                      }}
                    >
                      [{j + 1}] {meetingId ? '' : `${s.title} · `}
                      {fmtClock(s.startMs)}
                      {s.speaker ? ` · ${s.speaker}` : ''}
                    </button>
                  ))}
                </div>
              ) : null}
              <Text size="xs" muted style={{ marginTop: 6, display: 'block' }}>
                {x.answer.provider === 'extractive'
                  ? 'Quotes from your transcripts (add an LLM key in Settings for written answers)'
                  : `Answered by ${x.answer.provider}`}
              </Text>
            </Card>
          ),
        )}
        {busy ? <Spinner size={14} /> : null}
        {error ? (
          <Callout
            tone="danger"
            title="Couldn't answer that"
            data-testid="chat-error"
            action={
              lastUser ? (
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() =>
                    void ask(
                      lastUser.content,
                      turns.slice(0, turns.lastIndexOf(lastUser)),
                    )
                  }
                >
                  Retry
                </Button>
              ) : null
            }
          >
            {error}
          </Callout>
        ) : null}
        <div ref={endRef} />
      </div>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
        style={{ display: 'flex', gap: 8 }}
      >
        <Input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder={
            meetingId ? 'Ask about this meeting…' : 'Ask across all meetings…'
          }
          aria-label={
            meetingId ? 'Ask about this meeting' : 'Ask across all meetings'
          }
          data-testid="chat-input"
          maxLength={2000}
        />
        <Button
          type="submit"
          disabled={busy || !q.trim()}
          data-testid="chat-send"
        >
          Ask
        </Button>
      </form>
    </div>
  );
}
