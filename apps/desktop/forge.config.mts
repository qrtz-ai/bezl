import type { ForgeConfig } from '@electron-forge/shared-types';
import { MakerSquirrel } from '@electron-forge/maker-squirrel';
import { MakerZIP } from '@electron-forge/maker-zip';
import { MakerDeb } from '@electron-forge/maker-deb';
import { MakerRpm } from '@electron-forge/maker-rpm';
import { MakerDMG } from '@electron-forge/maker-dmg';
import { VitePlugin } from '@electron-forge/plugin-vite';
import { FusesPlugin } from '@electron-forge/plugin-fuses';
import { FuseV1Options, FuseVersion } from '@electron/fuses';
import fs from 'node:fs';
import path from 'node:path';

/** Swift helper built by `npm run build:helper` (macOS only). On Linux it is absent and the TS mock is used. */
const helperBin = path.resolve(
  import.meta.dirname,
  '../../native/qrtz-helper/.build/release/qrtz-helper',
);
const extraResource = fs.existsSync(helperBin) ? [helperBin] : [];

/**
 * macOS signing + notarization, only when scripts/release-mac.sh (or you) set the env.
 * Unset = unsigned local build, exactly as before. See docs/release.md.
 */
const env = process.env;
const entitlements = path.resolve(
  import.meta.dirname,
  'entitlements.mac.plist',
);
const osxSign = env.QRTZ_MAC_SIGN_IDENTITY
  ? {
      identity: env.QRTZ_MAC_SIGN_IDENTITY,
      // Hardened runtime + entitlements for the app, its helpers, the Swift helper and the unpacked onnxruntime addon.
      optionsForFile: () => ({ hardenedRuntime: true, entitlements }),
    }
  : undefined;
const osxNotarize = !osxSign
  ? undefined
  : env.QRTZ_NOTARY_PROFILE
    ? { keychainProfile: env.QRTZ_NOTARY_PROFILE }
    : env.APPLE_ID && env.APPLE_APP_SPECIFIC_PASSWORD && env.APPLE_TEAM_ID
      ? {
          appleId: env.APPLE_ID,
          appleIdPassword: env.APPLE_APP_SPECIFIC_PASSWORD,
          teamId: env.APPLE_TEAM_ID,
        }
      : undefined;

/**
 * Forge `packageAfterCopy` hook: the Vite plugin ships only `.vite/`, so the
 * on-device embedding runtime (kept external in vite.main.config.mts) is copied
 * into `<app>/node_modules` here, trimmed to the target platform/arch.
 *
 * - @huggingface/transformers: node entry points only (no web/WASM builds).
 * - onnxruntime-node: dist + bin/napi-v6/<platform>/<arch>, minus CUDA/TensorRT providers.
 * - onnxruntime-common: whole (1 MB).
 * - sharp: replaced by a stub; transformers imports it statically for image
 *   pipelines we never use, and the real one would drag in ~30 MB of libvips.
 *
 * onnxruntime-node 1.30 ships no darwin/x64 binary. On Intel Macs the worker falls back
 * to onnxruntime-web (WASM, bundled below); see src/worker/embedder.ts.
 */

const root = path.resolve(import.meta.dirname, '../../node_modules');
const SKIP_PROVIDERS = /onnxruntime_providers_(cuda|tensorrt)/;

function copy(
  src: string,
  dst: string,
  filter: (p: string) => boolean = () => true,
) {
  fs.cpSync(src, dst, { recursive: true, dereference: true, filter });
}

function bundleExternals(
  buildPath: string,
  platform: string,
  arch: string,
): void {
  const nm = path.join(buildPath, 'node_modules');
  fs.mkdirSync(nm, { recursive: true });

  const tf = path.join(root, '@huggingface/transformers');
  const tfDst = path.join(nm, '@huggingface/transformers');
  fs.mkdirSync(path.join(tfDst, 'dist'), { recursive: true });
  fs.copyFileSync(
    path.join(tf, 'package.json'),
    path.join(tfDst, 'package.json'),
  );
  for (const f of ['transformers.node.mjs', 'transformers.node.cjs']) {
    fs.copyFileSync(path.join(tf, 'dist', f), path.join(tfDst, 'dist', f));
  }

  copy(
    path.join(root, 'onnxruntime-common'),
    path.join(nm, 'onnxruntime-common'),
  );

  const ort = path.join(root, 'onnxruntime-node');
  const ortDst = path.join(nm, 'onnxruntime-node');
  fs.mkdirSync(ortDst, { recursive: true });
  fs.copyFileSync(
    path.join(ort, 'package.json'),
    path.join(ortDst, 'package.json'),
  );
  copy(
    path.join(ort, 'dist'),
    path.join(ortDst, 'dist'),
    (p) => !p.endsWith('.map'),
  );
  const bin = path.join(ort, 'bin/napi-v6', platform, arch);
  if (fs.existsSync(bin)) {
    copy(
      bin,
      path.join(ortDst, 'bin/napi-v6', platform, arch),
      (p) => !SKIP_PROVIDERS.test(p),
    );
  } else {
    console.warn(
      `[bundle-externals] no onnxruntime-node binary for ${platform}/${arch}; embeddings fall back to FTS`,
    );
  }

  // WASM fallback (Intel Macs / missing native binary): only the Node build + the plain SIMD wasm (~14 MB).
  const web = path.join(root, 'onnxruntime-web');
  const webDst = path.join(nm, 'onnxruntime-web');
  fs.mkdirSync(path.join(webDst, 'dist'), { recursive: true });
  fs.copyFileSync(
    path.join(web, 'package.json'),
    path.join(webDst, 'package.json'),
  );
  for (const f of [
    'ort.node.min.js',
    'ort.node.min.mjs',
    'ort-wasm-simd-threaded.mjs',
    'ort-wasm-simd-threaded.wasm',
  ]) {
    fs.copyFileSync(path.join(web, 'dist', f), path.join(webDst, 'dist', f));
  }
  const webCommon = path.join(web, 'node_modules/onnxruntime-common');
  if (fs.existsSync(webCommon)) {
    copy(
      webCommon,
      path.join(webDst, 'node_modules/onnxruntime-common'),
      (p) => !p.endsWith('.map'),
    );
  }

  const sharp = path.join(nm, 'sharp');
  fs.mkdirSync(sharp, { recursive: true });
  fs.writeFileSync(
    path.join(sharp, 'package.json'),
    JSON.stringify({
      name: 'sharp',
      version: '0.0.0-qrtz-stub',
      main: 'index.js',
    }),
  );
  fs.writeFileSync(
    path.join(sharp, 'index.js'),
    "module.exports = function sharp() { throw new Error('sharp is not bundled in qrtz'); };\n",
  );
}

const config: ForgeConfig = {
  packagerConfig: {
    // Native addons cannot be dlopen'd from inside the asar.
    asar: { unpack: '**/{*.node,*.dylib,*.so,*.so.*,*.wasm,ort-wasm*.mjs}' },
    name: 'qrtz',
    executableName: 'qrtz',
    appBundleId: 'ai.qrtz.desktop',
    appCategoryType: 'public.app-category.productivity',
    extraResource,
    ...(osxSign ? { osxSign } : {}),
    ...(osxNotarize ? { osxNotarize } : {}),
    extendInfo: {
      NSMicrophoneUsageDescription:
        'qrtz records your side of meetings to transcribe them on-device.',
      NSAudioCaptureUsageDescription:
        'qrtz captures meeting audio to transcribe it on-device.',
    },
  },
  rebuildConfig: {},
  hooks: {
    // The Vite plugin ships only `.vite/`; add the external embedding runtime.
    packageAfterCopy: async (
      _config,
      buildPath,
      _electronVersion,
      platform,
      arch,
    ) => {
      bundleExternals(buildPath, platform, arch);
    },
  },
  makers: [
    new MakerSquirrel({}),
    new MakerDMG({ name: 'qrtz', format: 'ULFO' }, ['darwin']),
    // With a self-hosted feed, the zip maker also writes RELEASES.json (merged with the live one) to upload next to the zip.
    new MakerZIP(
      env.QRTZ_UPDATE_FEED_URL
        ? {
            macUpdateManifestBaseUrl: env.QRTZ_UPDATE_FEED_URL.replaceAll(
              '{platform}',
              'darwin',
            ).replaceAll('{arch}', env.QRTZ_MAKE_ARCH ?? 'arm64'),
          }
        : {},
      ['darwin'],
    ),
    new MakerRpm({}),
    new MakerDeb({}),
  ],
  plugins: [
    new VitePlugin({
      // `build` can specify multiple entry builds, which can be Main process, Preload scripts, Worker process, etc.
      // If you are familiar with Vite configuration, it will look really familiar.
      build: [
        {
          // `entry` is just an alias for `build.lib.entry` in the corresponding file of `config`.
          entry: 'src/main.ts',
          config: 'vite.main.config.mts',
          target: 'main',
        },
        {
          // Electron utilityProcess: SQLite + heavy jobs.
          entry: 'src/worker.ts',
          config: 'vite.main.config.mts',
          target: 'main',
        },
        {
          entry: 'src/preload.ts',
          config: 'vite.preload.config.mts',
          target: 'preload',
        },
      ],
      renderer: [
        {
          name: 'main_window',
          config: 'vite.renderer.config.mts',
        },
      ],
    }),
    // Fuses are used to enable/disable various Electron functionality
    // at package time, before code signing the application
    new FusesPlugin({
      version: FuseVersion.V1,
      [FuseV1Options.RunAsNode]: false,
      [FuseV1Options.EnableCookieEncryption]: true,
      [FuseV1Options.EnableNodeOptionsEnvironmentVariable]: false,
      [FuseV1Options.EnableNodeCliInspectArguments]: false,
      [FuseV1Options.EnableEmbeddedAsarIntegrityValidation]: true,
      [FuseV1Options.OnlyLoadAppFromAsar]: true,
    }),
  ],
};

export default config;
