import {
  MOCK_DICTATION,
  ruleCleanup,
  type CleanupLlm,
  type WsFactory,
  type WsLike,
} from '@qrtz/core';

/**
 * Dev/e2e stand-ins (never used by packaged builds; the env vars are stripped there):
 * QRTZ_TALK_CLOUD=echo|fail|silent fakes the Deepgram socket; QRTZ_TALK_LLM=fake|hang fakes the cleanup LLM.
 */
export function fakeCloudWs(mode: string): WsFactory {
  return () => {
    let pushes = 0;
    const ws: WsLike = {
      readyState: 0,
      onopen: null,
      onmessage: null,
      onerror: null,
      onclose: null,
      send(data) {
        if (mode !== 'echo') return;
        const words = MOCK_DICTATION.split(' ');
        const reply = (
          transcript: string,
          isFinal: boolean,
          fromFinalize = false,
        ) =>
          setTimeout(
            () =>
              ws.onmessage?.({
                data: JSON.stringify({
                  type: 'Results',
                  is_final: isFinal,
                  from_finalize: fromFinalize,
                  channel: { alternatives: [{ transcript }] },
                }),
              }),
            5,
          );
        if (typeof data !== 'string')
          reply(
            words.slice(0, Math.min(++pushes, words.length)).join(' '),
            false,
          );
        else if (data.includes('Finalize')) reply(MOCK_DICTATION, true, true);
      },
      close() {
        (ws as { readyState: number }).readyState = 3;
      },
    };
    setTimeout(() => {
      if (mode === 'fail') {
        ws.onerror?.({});
        ws.onclose?.({ code: 1006, reason: 'fake failure' });
        return;
      }
      (ws as { readyState: number }).readyState = 1;
      ws.onopen?.({});
    }, 10);
    return ws;
  };
}

export function fakeCleanupLlm(mode: string): CleanupLlm {
  return (messages, signal) =>
    new Promise((resolve, reject) => {
      if (mode === 'hang') {
        signal.addEventListener('abort', () => reject(new Error('aborted')));
        return;
      }
      const raw = messages.at(-1)?.content ?? '';
      setTimeout(() => resolve(ruleCleanup(raw).replace(/^So,? /, '')), 20);
    });
}
