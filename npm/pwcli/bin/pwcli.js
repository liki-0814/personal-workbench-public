#!/usr/bin/env node

const { dirname, join } = require('node:path');
const { spawnSync } = require('node:child_process');

const launcherPackage = '@liki030814/pwcli';
const registry = 'https://registry.npmjs.org/';
const packages = {
  'darwin-arm64': 'pwcli-darwin-arm64',
};

const target = `${process.platform}-${process.arch}`;
const platformPackage = packages[target];

if (!platformPackage) {
  console.error(
    `pwcli does not yet publish a binary for ${target}. ` +
    'Build from source instead: https://github.com/liki-0814/personal-workbench-public#build-from-source',
  );
  process.exit(1);
}

function resolveExecutable() {
  const packageJson = require.resolve(`${platformPackage}/package.json`);
  return join(dirname(packageJson), 'bin', process.platform === 'win32' ? 'pwcli.exe' : 'pwcli');
}

let executable;
try {
  executable = resolveExecutable();
} catch (error) {
  console.error(
    `The optional package ${platformPackage} is missing. ` +
    'Reinstall without disabling optional dependencies.',
  );
  process.exit(1);
}

function daemonWasRunning() {
  const result = spawnSync(executable, ['daemon', 'status', '--json'], { encoding: 'utf8' });
  if (result.error || result.status !== 0) return false;
  try {
    const status = JSON.parse(result.stdout);
    return status.running === true && status.healthy === true;
  } catch {
    return false;
  }
}

function runUpgrade() {
  const restartDaemon = daemonWasRunning();
  if (restartDaemon) {
    const stop = spawnSync(executable, ['daemon', 'stop'], { stdio: 'inherit' });
    if (stop.error || stop.status !== 0) {
      console.error('Unable to stop the running pwcli daemon; update cancelled.');
      process.exit(stop.status ?? 1);
    }
  }

  const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
  const update = spawnSync(
    npm,
    [
      'install',
      '--global',
      `${launcherPackage}@latest`,
      `--registry=${registry}`,
      '--prefer-online',
    ],
    { stdio: 'inherit' },
  );
  if (update.error || update.status !== 0) {
    console.error(`Failed to update pwcli${update.error ? `: ${update.error.message}` : '.'}`);
    if (restartDaemon) spawnSync(executable, ['daemon', 'start'], { stdio: 'inherit' });
    process.exit(update.status ?? 1);
  }

  let updatedExecutable;
  try {
    updatedExecutable = resolveExecutable();
  } catch (error) {
    console.error(`pwcli was updated, but ${platformPackage} could not be resolved.`);
    process.exit(1);
  }
  if (restartDaemon) {
    const start = spawnSync(updatedExecutable, ['daemon', 'start'], { stdio: 'inherit' });
    if (start.error || start.status !== 0) process.exit(start.status ?? 1);
  }
  const version = spawnSync(updatedExecutable, ['--version'], { stdio: 'inherit' });
  process.exit(version.status ?? (version.error ? 1 : 0));
}

if (process.argv[2] === 'upgrade' || process.argv[2] === 'update') {
  runUpgrade();
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
