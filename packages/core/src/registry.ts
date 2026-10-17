import type { SubApp } from './subapp';

export class SubAppRegistry {
  private readonly apps = new Map<string, SubApp>();

  register(app: SubApp): void {
    const { id } = app.manifest;
    if (this.apps.has(id))
      throw new Error(`Sub app "${id}" already registered`);
    this.apps.set(id, app);
  }

  get(id: string): SubApp | undefined {
    return this.apps.get(id);
  }

  list(): SubApp[] {
    return [...this.apps.values()].sort(
      (a, b) =>
        (a.manifest.order ?? 100) - (b.manifest.order ?? 100) ||
        a.manifest.name.localeCompare(b.manifest.name),
    );
  }
}

export function createRegistry(apps: SubApp[] = []): SubAppRegistry {
  const registry = new SubAppRegistry();
  for (const app of apps) registry.register(app);
  return registry;
}
