// Talk pill window entry (pill.html): the transport and the dictation pill only. No shell, registry or
// sub apps (ADR 0090; window-bundles.test.ts checks the bundle).
import { createRoot } from 'react-dom/client';
import { installTransport } from './transport';
import { TalkPillApp } from './shell/TalkPill';
import { installRendererLogging } from './shell/rendererLog';
import { shellClient } from './shell/shellClient';

await installTransport();
installRendererLogging(shellClient(), 'pill');
createRoot(document.getElementById('root')!).render(<TalkPillApp />);
