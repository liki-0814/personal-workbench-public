#!/usr/bin/env node
/**
 * One-time migration from the legacy project .env to ~/.pwcli/config.json.
 *
 * Safety order: read + validate -> back up .env -> atomically write config ->
 * remove .env. The backup is never deleted by this script.
 */
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from 'fs';
import { dirname, join, resolve } from 'path';
import { fileURLToPath } from 'url';
import { homedir } from 'os';
import { parse as parseDotenv } from 'dotenv';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ENV_PATH = process.env.PWB_MIGRATION_ENV_PATH || resolve(ROOT, '.env');
const CONFIG_PATH = process.env.PWB_MIGRATION_CONFIG_PATH || join(homedir(), '.pwcli/config.json');

const FIELD_MAP = [
  ['BACKEND_PORT', ['server', 'backendPort'], 'int'],
  ['VITE_BACKEND_URL', ['frontend', 'backendUrl']],
  ['PWB_DATA_DIR', ['server', 'dataDir']],
  ['PWB_SHOW_HIDDEN_FILES', ['server', 'showHiddenFiles'], 'bool'],
  ['PWB_BACKUP_INTERVAL_MS', ['server', 'backupIntervalMs'], 'int'],
  ['VITE_FAVICON_SERVICE_URL', ['frontend', 'faviconServiceUrl']],
  ['MINERU_TOKEN', ['ai', 'mineruToken']],
  ['PWB_FS_BASE', ['tools', 'fsBase']],
  ['PWCLI_CODE_AGENT_BACKEND', ['tools', 'codeAgent', 'backend']],
  ['PWCLI_CODE_AGENT_MODEL', ['tools', 'codeAgent', 'model']],
  ['PWCLI_CODE_AGENT_CONTEXT_WINDOW', ['tools', 'codeAgent', 'contextWindow'], 'int'],
  ['PWCLI_CODE_AGENT_EFFORT', ['tools', 'codeAgent', 'effort']],
  ['PWCLI_CODE_AGENT_THINKING', ['tools', 'codeAgent', 'thinking']],
  ['PWCLI_CODE_AGENT_FAST', ['tools', 'codeAgent', 'fast'], 'bool'],
  ['PWCLI_MEMORY_INJECTION_MODE', ['memory', 'injectionMode']],
  ['PWCLI_MEMORY_AUTO_RETRIEVE', ['memory', 'autoRetrieve'], 'bool'],
  ['PWCLI_MEMORY_MAX_RESULTS', ['memory', 'maxResults'], 'int'],
  ['PWCLI_MEMORY_MAX_DIGEST_BYTES', ['memory', 'maxDigestBytes'], 'int'],
  ['PWCLI_AUTO_MEMORY_EXTRACT', ['features', 'autoMemoryExtract'], 'bool'],
];

const mappedKeys = new Set(FIELD_MAP.map(([key]) => key));

function readConfig() {
  if (!existsSync(CONFIG_PATH)) return { schemaVersion: 1 };
  const parsed = JSON.parse(readFileSync(CONFIG_PATH, 'utf8'));
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error(`${CONFIG_PATH} must contain a JSON object`);
  }
  return parsed;
}

function setNested(obj, path, value) {
  let current = obj;
  for (const key of path.slice(0, -1)) {
    if (!current[key] || typeof current[key] !== 'object' || Array.isArray(current[key])) {
      current[key] = {};
    }
    current = current[key];
  }
  current[path.at(-1)] = value;
}

function parseValue(raw, kind) {
  if (kind === 'int') {
    const value = Number.parseInt(raw, 10);
    if (!Number.isFinite(value)) throw new Error(`invalid integer: ${raw}`);
    return value;
  }
  if (kind === 'bool') return /^(1|true|yes|on)$/i.test(raw.trim());
  if (kind === 'json') return JSON.parse(raw);
  return raw;
}

function parseModels(raw) {
  if (!raw) return [];
  let values;
  try {
    const parsed = JSON.parse(raw);
    values = Array.isArray(parsed) ? parsed : [parsed];
  } catch {
    values = raw.split(',');
  }
  return values.map((entry) => {
    if (typeof entry === 'string') {
      const id = entry.trim();
      return id ? { id, name: id } : null;
    }
    if (!entry || typeof entry !== 'object' || !entry.id) return null;
    return { ...entry, id: String(entry.id), name: String(entry.name || entry.id) };
  }).filter(Boolean);
}

function parseProviders(env) {
  const indexes = [...new Set(Object.keys(env).flatMap((key) => {
    const match = key.match(/^VITE_AI_PROVIDER_(\d+)_/);
    return match ? [Number(match[1])] : [];
  }))].sort((a, b) => a - b);

  return indexes.flatMap((index) => {
    const prefix = `VITE_AI_PROVIDER_${index}_`;
    const name = env[`${prefix}NAME`]?.trim();
    const baseUrl = env[`${prefix}BASE_URL`]?.trim();
    const apiKey = env[`${prefix}API_KEY`]?.trim();
    if (!name && !baseUrl && !apiKey) return [];
    const models = parseModels(env[`${prefix}MODELS`] || '');
    const model = env[`${prefix}MODEL`]?.trim() || models[0]?.id || '';
    return [{
      name: name || `Provider ${index + 1}`,
      base_url: baseUrl || '',
      api_key: apiKey || '',
      protocol: env[`${prefix}PROTOCOL`]?.trim() || 'openai',
      model,
      models,
    }];
  });
}

function expandHome(value) {
  if (!value || value === '~') return value === '~' ? homedir() : value;
  return value.startsWith('~/') ? join(homedir(), value.slice(2)) : value;
}

function timestamp() {
  return new Date().toISOString().replace(/[:.]/g, '').replace('T', '-').replace('Z', '');
}

function writeConfigAtomic(config) {
  mkdirSync(dirname(CONFIG_PATH), { recursive: true });
  const temp = `${CONFIG_PATH}.${process.pid}.tmp`;
  writeFileSync(temp, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600 });
  chmodSync(temp, 0o600);
  renameSync(temp, CONFIG_PATH);
  chmodSync(CONFIG_PATH, 0o600);
}

function main() {
  if (!existsSync(ENV_PATH)) {
    console.log('[migrate] legacy .env not found; nothing to migrate');
    return;
  }

  const envText = readFileSync(ENV_PATH, 'utf8');
  const env = parseDotenv(Buffer.from(envText));
  const config = readConfig();
  config.schemaVersion ??= 1;

  const dataDir = expandHome(env.PWB_DATA_DIR || config.server?.dataDir || '~/.pwcli/data');
  const backupDir = join(dataDir, 'env-backups');
  const backupPath = join(backupDir, `env-${timestamp()}.pre-config-json-migration.bak`);
  mkdirSync(backupDir, { recursive: true });
  copyFileSync(ENV_PATH, backupPath);
  chmodSync(backupPath, 0o600);

  for (const [key, path, kind] of FIELD_MAP) {
    if (env[key] == null || env[key] === '') continue;
    setNested(config, path, parseValue(env[key], kind));
  }
  if (env.VITE_BACKEND_URL) config.backend_url = env.VITE_BACKEND_URL;

  const providers = parseProviders(env);
  if (providers.length > 0) {
    config.providers = providers;
    const names = new Set(providers.map((provider) => provider.name));
    if (!names.has(config.active_provider)) config.active_provider = providers[0].name;
  }

  const providerKeyPattern = /^VITE_AI_PROVIDER_\d+_/;
  const unknownEntries = Object.entries(env).filter(([key]) => (
    !mappedKeys.has(key) && !providerKeyPattern.test(key)
  ));
  if (unknownEntries.length > 0) {
    config.legacyEnvironment = {
      ...(config.legacyEnvironment || {}),
      ...Object.fromEntries(unknownEntries),
    };
  }

  writeConfigAtomic(config);
  unlinkSync(ENV_PATH);

  console.log(`[migrate] backup created: ${backupPath}`);
  console.log(`[migrate] migrated ${Object.keys(env).length} keys into ${CONFIG_PATH}`);
  console.log('[migrate] legacy .env removed after successful atomic write');
}

try {
  main();
} catch (error) {
  console.error(`[migrate] failed: ${error.message}`);
  process.exitCode = 1;
}
