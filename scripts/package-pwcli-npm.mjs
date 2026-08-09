import { chmodSync, copyFileSync, mkdirSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const target = `${process.platform}-${process.arch}`;
const supported = {
  'darwin-arm64': {
    packageDir: path.join(root, 'npm', 'pwcli-darwin-arm64'),
    binary: path.join(root, 'pwcli', 'target', 'release', 'pwcli'),
    output: 'pwcli',
  },
};

const config = supported[target];
if (!config) {
  throw new Error(`No npm binary package is configured for ${target}`);
}

const outputDir = path.join(root, 'npm', 'dist');
const npmCache = path.join(root, 'npm', '.cache');
const packageBinDir = path.join(config.packageDir, 'bin');
const packagedBinary = path.join(packageBinDir, config.output);

mkdirSync(packageBinDir, { recursive: true });
mkdirSync(outputDir, { recursive: true });
mkdirSync(npmCache, { recursive: true });
rmSync(packagedBinary, { force: true });
copyFileSync(config.binary, packagedBinary);
chmodSync(packagedBinary, 0o755);

execFileSync(packagedBinary, ['--version'], { stdio: 'inherit' });
execFileSync('npm', ['pack', config.packageDir, '--pack-destination', outputDir], {
  cwd: root,
  env: { ...process.env, npm_config_cache: npmCache },
  stdio: 'inherit',
});
execFileSync('npm', ['pack', path.join(root, 'npm', 'pwcli'), '--pack-destination', outputDir], {
  cwd: root,
  env: { ...process.env, npm_config_cache: npmCache },
  stdio: 'inherit',
});

rmSync(packagedBinary, { force: true });
console.log(`Created npm tarballs in ${outputDir}`);
