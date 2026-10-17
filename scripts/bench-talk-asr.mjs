#!/usr/bin/env node
// Measures streaming ASR latency for qrtz talk's cloud engines with a real key and a real speech WAV.
// Streams 16 kHz mono PCM16 at real time in 100 ms frames (like the helper), then reports:
//   first partial  = audio start → first non-empty partial
//   final          = end of audio → final text after the flush (Finalize / commit) — the "key-up → text" ASR cost
// Usage: QRTZ_SECRET_ASR_DEEPGRAM=… QRTZ_SECRET_ASR_ELEVENLABS=… bun scripts/bench-talk-asr.mjs speech16k.wav [runs]
// (Convert any file: ffmpeg -i in.m4a -ar 16000 -ac 1 -c:a pcm_s16le speech16k.wav)
import fs from 'node:fs';

const [wavPath, runsArg] = process.argv.slice(2);
if (!wavPath) {
  console.error('usage: bun scripts/bench-talk-asr.mjs speech16k.wav [runs]');
  process.exit(2);
}
const wav = fs.readFileSync(wavPath);
const pcm = wav.subarray(wav.indexOf('data') + 8);
const FRAME = 3200; // 100 ms
const runs = Number(runsArg ?? 3);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const engines = {
  deepgram: {
    key: process.env.QRTZ_SECRET_ASR_DEEPGRAM,
    open: (key) =>
      new WebSocket(
        'wss://api.deepgram.com/v1/listen?model=nova-3&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true',
        ['token', key],
      ),
    frame: (b) => b,
    flush: () => JSON.stringify({ type: 'Finalize' }),
    parse: (m) =>
      m.type === 'Results'
        ? {
            text: m.channel?.alternatives?.[0]?.transcript ?? '',
            final: !!m.from_finalize,
          }
        : null,
  },
  elevenlabs: {
    key: process.env.QRTZ_SECRET_ASR_ELEVENLABS,
    open: (key) =>
      new WebSocket(
        'wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&audio_format=pcm_16000&commit_strategy=manual',
        { headers: { 'xi-api-key': key } },
      ),
    frame: (b) =>
      JSON.stringify({
        message_type: 'input_audio_chunk',
        audio_base_64: Buffer.from(b).toString('base64'),
        commit: false,
        sample_rate: 16000,
      }),
    flush: () =>
      JSON.stringify({
        message_type: 'input_audio_chunk',
        audio_base_64: '',
        commit: true,
        sample_rate: 16000,
      }),
    parse: (m) =>
      m.message_type === 'partial_transcript'
        ? { text: m.text ?? '', final: false }
        : m.message_type?.startsWith('committed_transcript')
          ? { text: m.text ?? '', final: true }
          : null,
  },
};

async function once(name, e) {
  const ws = e.open(e.key);
  ws.binaryType = 'arraybuffer';
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = () => rej(new Error('connect failed'));
  });
  let t0 = 0,
    firstPartial,
    endAt = 0,
    finalMs,
    finalText = '';
  const done = new Promise((res) => {
    ws.onmessage = (ev) => {
      const r = e.parse(JSON.parse(String(ev.data)));
      if (!r) return;
      if (r.text && firstPartial === undefined)
        firstPartial = performance.now() - t0;
      if (r.final && endAt) {
        finalMs = performance.now() - endAt;
        finalText = r.text;
        res();
      }
    };
    setTimeout(res, 10_000);
  });
  t0 = performance.now();
  for (let i = 0; i < pcm.length; i += FRAME) {
    ws.send(e.frame(pcm.subarray(i, i + FRAME)));
    await sleep(100);
  }
  endAt = performance.now();
  ws.send(e.flush());
  await done;
  ws.close();
  return { firstPartial, finalMs, finalText };
}

for (const [name, e] of Object.entries(engines)) {
  if (!e.key) {
    console.log(`${name}: skipped (no key)`);
    continue;
  }
  for (let i = 0; i < runs; i++) {
    try {
      const r = await once(name, e);
      console.log(
        `${name} run ${i + 1}: first partial ${r.firstPartial?.toFixed(0) ?? '-'} ms, final after end ${r.finalMs?.toFixed(0) ?? 'timeout'} ms — "${r.finalText.slice(0, 60)}"`,
      );
    } catch (err) {
      console.log(`${name} run ${i + 1}: ${err.message}`);
    }
  }
}
