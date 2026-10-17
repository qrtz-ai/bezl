import { createRoot } from 'react-dom/client';
import { App } from './shell/App';
import { MiniHudApp } from './shell/MiniHud';
import { TalkPillApp } from './shell/TalkPill';

const root = createRoot(document.getElementById('root')!);
const hash = window.location.hash;
root.render(
  hash === '#hud' ? (
    <MiniHudApp />
  ) : hash === '#pill' ? (
    <TalkPillApp />
  ) : (
    <App />
  ),
);
