// HUD window entry (hud.html): the transport and the floating recording HUD only. No shell, registry or
// sub apps (ADR 0088; window-bundles.test.ts checks the bundle).
import { createRoot } from 'react-dom/client';
import { installTransport } from './transport';
import { MiniHudApp } from './shell/MiniHud';
import { installRendererLogging } from './shell/rendererLog';
import { shellClient } from './shell/shellClient';

await installTransport();
installRendererLogging(shellClient(), 'hud');
createRoot(document.getElementById('root')!).render(<MiniHudApp />);
