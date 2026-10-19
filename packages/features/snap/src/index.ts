import { defineSubApp } from '@qrtz/core';
import { manifest } from './manifest';

export default defineSubApp({ manifest, load: () => import('./SnapView') });
