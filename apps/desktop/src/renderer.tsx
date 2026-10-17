import { createRoot } from 'react-dom/client';
import { App } from './shell/App';
import { MiniHudApp } from './shell/MiniHud';
import { TalkPillApp } from './shell/TalkPill';
import { installRendererLogging } from './shell/rendererLog';
import { shellClient } from './shell/shellClient';

const root = createRoot(document.getElementById('root')!);
const hash = window.location.hash;
// ADR 0083: uncaught errors, unhandled rejections and player errors from this window go to the local log.
installRendererLogging(
  shellClient(),
  hash === '#hud' ? 'hud' : hash === '#pill' ? 'pill' : 'main',
);
root.render(
  hash === '#hud' ? (
    <MiniHudApp />
  ) : hash === '#pill' ? (
    <TalkPillApp />
  ) : (
    <App />
  ),
);
