import { app } from 'electron';
import started from 'electron-squirrel-startup';
import { bootstrap } from './main/index';

// Handle creating/removing shortcuts on Windows when installing/uninstalling.
if (started) app.quit();
else bootstrap();
