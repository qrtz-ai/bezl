import { createRoot } from 'react-dom/client';
import { App } from './shell/App';
import { installRendererLogging } from './shell/rendererLog';
import { shellClient } from './shell/shellClient';

// ADR 0083: uncaught errors, unhandled rejections and player errors from this window go to the local log.
installRendererLogging(shellClient(), 'main');
createRoot(document.getElementById('root')!).render(<App />);
