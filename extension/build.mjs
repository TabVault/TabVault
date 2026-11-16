// Builds the TabVault extension for Chrome, Edge and Firefox.
//
// Output: dist/<browser>/ — a load-unpacked directory per browser, with the
// right manifest baked in (MV3 background shape differs: Chrome/Edge use a
// module service_worker, Firefox uses an MV2-style background scripts array).
//
// Usage: node build.mjs [chrome|edge|firefox]   (default: all three)

import { build } from 'esbuild';
import {
  cpSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
  existsSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(fileURLToPath(import.meta.url));
const dist = join(root, 'dist');
const ALL = ['chrome', 'edge', 'firefox'];
const targets = process.argv.slice(2);
const browsers = targets.length ? targets : ALL;

// The Chrome/Edge extension ID is pinned by this public key, so the native
// host manifest's allowed_origins matches on every machine. Never commit the
// matching private key.
const CHROME_KEY =
  'MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA4nJF+XgzUt7lQNO5R3XJ77XdkmZow2nGpGxIp8fC5mzKliWJHZcHHQU6Hrabtiwt78LUiQydIFJpbHVixq5gYEdKbXnVmEg+ayo3KssizACcrLgy/OX7avl2UyhOJbQpjfahxtLhWca5ukcSNGK27xHPuJnIDG06M86ogWPtupD4YvxNGAogy1LkS6gtsAMQd0w2KG+TKO2o14HZxNA5OxvmDkc8Rr98yAzdQlO4P0awRubcvAVYvWsgJBvWZ+7dfwIG2NwbwsVTVYVd5UE9vZT/2G8LmTzx6+wVWJvnNsfF7yBbVHX5nLbZj1ajn83Moup8z73FkC6wDnXthICjdQIDAQAB';
const CHROME_EXTENSION_ID = 'fbeikljjdhlgipmpgeohjigbgdmnkeae';
const FIREFOX_ADDON_ID = 'tabvault@tabvault';

const permissions = [
  'tabs',
  'tabGroups',
  'storage',
  'nativeMessaging',
  'alarms',
  'notifications',
  'history',
  'sessions',
];

const shared = {
  manifest_version: 3,
  name: 'TabVault',
  version: '1.0.0',
  description:
    'TabVault continuously records your tabs and windows to a local, offline database so you never lose them again.',
  icons: {
    16: 'icons/icon-16.png',
    32: 'icons/icon-32.png',
    48: 'icons/icon-48.png',
    128: 'icons/icon-128.png',
  },
  action: {
    default_title: 'TabVault',
    default_popup: 'popup.html',
    default_icon: {
      16: 'icons/icon-16.png',
      32: 'icons/icon-32.png',
      48: 'icons/icon-48.png',
      128: 'icons/icon-128.png',
    },
  },
  options_ui: {
    page: 'options.html',
    open_in_tab: true,
  },
  commands: {
    '_execute_action': {
      suggested_key: { default: 'Alt+Shift+V' },
      description: 'Open TabVault',
    },
  },
  permissions,
};

const manifests = {
  chrome: {
    ...shared,
    key: CHROME_KEY,
    background: { service_worker: 'background.js', type: 'module' },
    minimum_chrome_version: '120',
  },
  edge: {
    ...shared,
    key: CHROME_KEY,
    background: { service_worker: 'background.js', type: 'module' },
    minimum_chrome_version: '120',
  },
  firefox: {
    ...shared,
    browser_specific_settings: {
      gecko: {
        id: FIREFOX_ADDON_ID,
        strict_min_version: '115.0',
      },
    },
    background: { scripts: ['background.js'], type: 'module' },
    // Firefox honours this for its own permission prompt ordering.
    permissions,
  },
};

function buildManifest(browser) {
  return JSON.stringify(manifests[browser], null, 2) + '\n';
}

async function main() {
  for (const browser of browsers) {
    const outDir = join(dist, browser);
    rmSync(outDir, { recursive: true, force: true });
    mkdirSync(outDir, { recursive: true });

    await build({
      entryPoints: [
        join(root, 'src/background.ts'),
        join(root, 'src/popup.ts'),
        join(root, 'src/options.ts'),
      ],
      outdir: outDir,
      bundle: true,
      format: browser === 'firefox' ? 'esm' : 'esm',
      target: browser === 'firefox' ? 'firefox115' : 'chrome120',
      platform: 'browser',
      sourcemap: false,
      logLevel: 'silent',
      define: {
        __TABVAULT_BROWSER__: JSON.stringify(browser),
      },
    });

    for (const asset of ['popup.html', 'options.html']) {
      cpSync(join(root, asset), join(outDir, asset));
    }
    cpSync(join(root, 'icons'), join(outDir, 'icons'), { recursive: true });
    writeFileSync(join(outDir, 'manifest.json'), buildManifest(browser));

    console.log(`built ${browser} -> ${outDir}`);
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
