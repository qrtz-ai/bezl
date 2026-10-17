/** Names of well-known secrets. Values never leave main/worker except to the provider. */
export const SECRET_NAMES = [
  'llm.openai',
  'llm.anthropic',
  'llm.google',
  'llm.openrouter',
  'asr.nvidia',
  'hf.token',
] as const;
export type SecretName = (typeof SECRET_NAMES)[number] | (string & {});

export interface SecretStore {
  get(name: SecretName): Promise<string | undefined>;
  set(name: SecretName, value: string | null): Promise<void>;
  /** Names only, never values. */
  list(): Promise<string[]>;
}

export class MemorySecretStore implements SecretStore {
  private m = new Map<string, string>();
  async get(n: string) {
    return this.m.get(n);
  }
  async set(n: string, v: string | null) {
    if (v === null) this.m.delete(n);
    else this.m.set(n, v);
  }
  async list() {
    return [...this.m.keys()].sort();
  }
}

/** Env override: QRTZ_SECRET_LLM_OPENAI etc. Useful for CI and headless runs. */
export const envNameFor = (name: string) =>
  'QRTZ_SECRET_' + name.toUpperCase().replace(/[^A-Z0-9]/g, '_');

/** Env names checked for a secret, e.g. QRTZ_SECRET_LLM_OPENROUTER and the short QRTZ_SECRET_OPENROUTER. */
export const envNamesFor = (name: string): string[] => {
  const names = [envNameFor(name)];
  if (name.startsWith('llm.')) names.push(envNameFor(name.slice(4)));
  return names;
};
