import type { ModuleId } from './modules';

/**
 * PostHog flags that show a module's app in the qrtz app, dev builds included (sidebar, apps page); a flag that is
 * off hides it. The browser mock, tests and Storybook show every app and never call PostHog.
 */
export const MODULE_FLAGS: Readonly<Partial<Record<ModuleId, string>>> = {
  nano: 'app-nano',
  ctrl: 'app-ctrl',
};

const FLAGS_URL = 'https://us.i.posthog.com/flags?v=2';
// A PostHog project key is public by design: it can send events and read flags, nothing else.
const PROJECT_KEY = 'phc_yMoAAa3n6T9tWaDSzjqWexpP7gQqKo3fxTe7XS9TD2CT';
const ON_KEY = 'qrtz.flagsOn';
const ID_KEY = 'qrtz.flagsId';

// Any real qrtz window, dev included; the browser mock, Storybook and tests have no Tauri runtime.
const applies = () =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Unsaved: the next launch asks again.
  }
}

/** The modules whose flag is off, from PostHog's last answer (every flagged module before the first one). */
export function hiddenModules(): ModuleId[] {
  if (!applies()) return [];
  let on: unknown;
  try {
    on = JSON.parse(read(ON_KEY) ?? '[]');
  } catch {
    on = [];
  }
  return (Object.keys(MODULE_FLAGS) as ModuleId[]).filter(
    (m) => !(Array.isArray(on) && on.includes(MODULE_FLAGS[m])),
  );
}

let asked: Promise<ModuleId[]> | undefined;

/** Asks PostHog once per launch. Offline or a failed answer keeps the last one. */
export function loadHiddenModules(): Promise<ModuleId[]> {
  if (!applies()) return Promise.resolve([]);
  return (asked ??= (async () => {
    let id = read(ID_KEY);
    if (!id) write(ID_KEY, (id = crypto.randomUUID()));
    const keys = Object.values(MODULE_FLAGS);
    try {
      const res = await fetch(FLAGS_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          api_key: PROJECT_KEY,
          distinct_id: id,
          flag_keys_to_evaluate: keys,
        }),
      });
      const body = (await res.json()) as {
        flags?: Record<string, { enabled?: boolean }>;
        errorsWhileComputingFlags?: boolean;
      };
      if (res.ok && body.flags && !body.errorsWhileComputingFlags)
        write(
          ON_KEY,
          JSON.stringify(keys.filter((k) => body.flags?.[k]?.enabled === true)),
        );
    } catch {
      // Offline: keep the last answer.
    }
    return hiddenModules();
  })());
}
