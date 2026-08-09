#!/usr/bin/env node

const { dirname, join } = require('node:path');
const { spawnSync } = require('node:child_process');

const packages = {
  'darwin-arm64': 'pwcli-darwin-arm64',
};

const target = `${process.platform}-${process.arch}`;
const packageName = packages[target];

if (!packageName) {
  console.error(
    `pwcli does not yet publish a binary for ${target}. ` +
    'Build from source instead: https://github.com/liki-0814/personal-workbench-public#build-from-source',
  );
  process.exit(1);
}

let executable;
try {
  const packageJson = require.resolve(`${packageName}/package.json`);
  executable = join(dirname(packageJson), 'bin', process.platform === 'win32' ? 'pwcli.exe' : 'pwcli');
} catch (error) {
  console.error(
    `The optional package ${packageName} is missing. ` +
    'Reinstall without disabling optional dependencies.',
  );
  process.exit(1);
}

const result = spawnSync(executable, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error(`Failed to start pwcli: ${result.error.message}`);
  process.exit(1);
}

if (result.signal) {
  process.kill(process.pid, result.signal);
} else {
  process.exit(result.status ?? 1);
}
