import { createRoot } from 'react-dom/client';
import { App } from './shell/App';
import { MiniHudApp } from './shell/MiniHud';

const root = createRoot(document.getElementById('root')!);
root.render(window.location.hash === '#hud' ? <MiniHudApp /> : <App />);
