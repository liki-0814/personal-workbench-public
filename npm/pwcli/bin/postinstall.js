#!/usr/bin/env node

const { dirname, join } = require('node:path');
const { spawnSync } = require('node:child_process');
const { version: installedVersion } = require('../package.json');

// A project dependency or transient `npx` install must never mutate the user's
// long-running global daemon. The upgrade self-heal is intentionally global-only.
if (process.env.npm_config_global !== 'true') process.exit(0);

const packages = {
  'darwin-arm64': 'pwcli-darwin-arm64',
};

const platformPackage = packages[`${process.platform}-${process.arch}`];
if (!platformPackage) process.exit(0);

let executable;
try {
  executable = join(
    dirname(require.resolve(`${platformPackage}/package.json`)),
    'bin',
    process.platform === 'win32' ? 'pwcli.exe' : 'pwcli',
  );
} catch {
  // npm may run lifecycle scripts before an optional platform package is ready.
  process.exit(0);
}

const statusResult = spawnSync(executable, ['daemon', 'status', '--json'], {
  encoding: 'utf8',
});
if (statusResult.error || statusResult.status !== 0) process.exit(0);

let status;
try {
  status = JSON.parse(statusResult.stdout);
} catch {
  process.exit(0);
}

const runningVersion = typeof status.version === 'string'
  ? status.version.trim().split(/\s+/, 1)[0]
  : '';
if (!status.running || !status.healthy || runningVersion === installedVersion) process.exit(0);

const restart = spawnSync(executable, ['daemon', 'restart'], { stdio: 'inherit' });
if (restart.error || restart.status !== 0) {
  console.warn(
    'pwcli was installed, but the previous daemon could not be restarted. ' +
    'Run `pwcli daemon restart` once.',
  );
}
