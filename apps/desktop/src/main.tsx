// Main window entry (index.html): install the transport, then boot the shell. The HUD is its own page
// (hud.html) and never loads the shell, its router or the sub apps (ADR 0090); echo's pill is native (ADR 0092).
import { installTransport } from './transport';

await installTransport();
await import('./renderer');
