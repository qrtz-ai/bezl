import { useState } from 'react';
import {
  Button,
  Callout,
  ChatAnswer as AnswerCard,
  ChatQuestion,
  Composer,
  Spinner,
  Stack,
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
  const [turns, setTurns] = useState<Turn[]>([]);
  const [q, setQ] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ask = async (question: string, prior: Turn[]) => {
    setBusy(true);
    setError(null);
    setTurns([...prior, { role: 'user', content: question }]);
    try {
      const answer = await qrtz.invoke('note:chat', {
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
    <Stack gap={3} data-testid="chat">
      <Stack gap={3} role="log" aria-live="polite">
        {turns.map((x, i) =>
          x.role === 'user' ? (
            <ChatQuestion key={i}>{x.content}</ChatQuestion>
          ) : (
            <AnswerCard
              key={i}
              text={x.content}
              sources={x.answer.sources.slice(0, 6).map((s, j) => ({
                label: `[${j + 1}] ${meetingId ? '' : `${s.title} · `}${fmtClock(s.startMs)}${
                  s.speaker ? ` · ${s.speaker}` : ''
                }`,
                title: s.text,
                onOpen: () => onOpenSource?.(s.meetingId, s.startMs),
              }))}
              {...(x.answer.provider === 'extractive'
                ? {}
                : { footer: x.answer.provider })}
              notice={
                x.answer.llmError ? (
                  <Callout
                    tone="warning"
                    title={x.answer.llmError}
                    data-testid="chat-llm-error"
                    action={
                      <Button
                        size="sm"
                        variant="secondary"
                        disabled={busy}
                        onClick={() => {
                          const prior = turns.slice(0, i - 1);
                          const question = turns[i - 1]?.content;
                          if (question) void ask(question, prior);
                        }}
                        data-testid="chat-retry"
                      >
                        retry
                      </Button>
                    }
                  />
                ) : undefined
              }
            />
          ),
        )}
        {busy ? <Spinner size={14} /> : null}
        {error ? (
          <Callout
            tone="danger"
            title={error}
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
                  retry
                </Button>
              ) : null
            }
          />
        ) : null}
      </Stack>
      <Composer
        value={q}
        onChange={setQ}
        onSubmit={submit}
        busy={busy}
        placeholder={
          meetingId ? 'ask about this meeting' : 'ask across all meetings'
        }
        label={meetingId ? 'ask about this meeting' : 'ask across all meetings'}
        inputTestId="chat-input"
        submitTestId="chat-send"
      />
    </Stack>
  );
}
