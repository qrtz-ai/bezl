import { createRegistry } from '@qrtz/core';
import home from '@qrtz/feature-home';
import settings from '@qrtz/feature-settings';

/** The only place the shell knows about individual sub apps.
 * To add one: create packages/features/<name>, add it to
 * apps/desktop/package.json dependencies, and list it here. */
export const registry = createRegistry([home, settings]);
