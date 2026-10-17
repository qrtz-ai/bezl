// Main window entry (index.html): install the transport, then boot the shell. The HUD and the talk pill
// are their own pages (hud.html, pill.html) and never load the shell, its router or the sub apps (ADR 0090).
import { installTransport } from './transport';

await installTransport();
await import('./renderer');
