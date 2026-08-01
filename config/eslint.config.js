import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'

// Forbid direct localStorage writes outside the storage layer.
// Today's class of bug: setProviders / setFeatureModel bypassed save() and
// wrote raw to localStorage, so config never reached the backend or other
// browsers. This rule guarantees synced data flows through @/core/storage's
// save()/remove(); intentionally local-only state must opt out with an
// eslint-disable comment so the bypass is auditable.
const noDirectLocalStorageWrite = {
  selector: "CallExpression[callee.object.name='localStorage'][callee.property.name=/^(setItem|removeItem|clear)$/]",
  message:
    "Use save()/remove() from @/core/storage instead of writing localStorage directly. " +
    "Direct writes bypass backend sync. For intentional local-only state, add " +
    "// eslint-disable-next-line no-restricted-syntax -- <reason> with justification.",
};

export default tseslint.config(
  { ignores: ['coverage', 'dist', 'e2e', 'temp_dir'] },
  {
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      'react-refresh/only-export-components': [
        'warn',
        { allowConstantExport: true },
      ],
      'no-restricted-syntax': ['error', noDirectLocalStorageWrite],
      // 接纳 `_` 前缀作为「故意不用」约定（TS/ESLint 生态默认推荐）。
      // 覆盖范围：参数、解构 rename（如 `const { x: _omit, ...rest }`）、catch 子句。
      'no-unused-vars': 'off',
      '@typescript-eslint/no-unused-vars': ['error', {
        argsIgnorePattern: '^_',
        varsIgnorePattern: '^_',
        caughtErrorsIgnorePattern: '^_',
        destructuredArrayIgnorePattern: '^_',
      }],
    },
  },
  {
    // Storage infrastructure implements the wrapper itself.
    files: ['src/core/storage/**/*.{ts,tsx}'],
    rules: { 'no-restricted-syntax': 'off' },
  },
  {
    // Tests prime jsdom localStorage directly for setup/teardown.
    files: ['**/*.test.{ts,tsx}', 'tests/**/*.{ts,tsx}'],
    rules: { 'no-restricted-syntax': 'off' },
  },
  {
    files: ['src/shell/**/*.{ts,tsx}'],
    rules: {
      'no-restricted-imports': ['error', {
        paths: [
          { name: '@/shell', message: 'Shell internals must import sibling modules directly instead of re-entering the shell barrel.' },
          { name: '@/types', message: 'Shell internals must import app types from @/types/app and shell types from ../types.' },
        ],
      }],
    },
  },
  {
    files: ['src/core/config/**/*.{ts,tsx}'],
    rules: {
      'no-restricted-imports': ['error', {
        paths: [
          { name: '@/core/storage', message: 'Config internals must import the concrete storage module to avoid the config/storage barrel cycle.' },
        ],
      }],
    },
  },
  {
    files: ['src/core/llm/{anthropic,openai}.ts'],
    rules: {
      'no-restricted-imports': ['error', {
        paths: [
          { name: './client', message: 'Protocol adapters must not import the client that composes them.' },
        ],
      }],
    },
  },
)
