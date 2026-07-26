import eslint from '@eslint/js';
import jsxA11y from 'eslint-plugin-jsx-a11y';
import react from 'eslint-plugin-react';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import globals from 'globals';
import tseslint from 'typescript-eslint';

const restrictedReferencePatterns = [
  {
    group: [
      'reference',
      'reference/**',
      '../reference/**',
      '../../reference/**',
      '**/reference/**',
    ],
    message: 'reference/ is immutable design material and may never be imported.',
  },
];

export default tseslint.config(
  {
    ignores: [
      'node_modules/**',
      'app/out/**',
      'out/**',
      'release/**',
      'coverage/**',
      'tmp/**',
      'pnpm-lock.yaml',
      'scripts/**/*.d.mts',
    ],
  },
  eslint.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  ...tseslint.configs.stylisticTypeChecked,
  {
    files: ['**/*.{js,mjs,cjs}'],
    ...tseslint.configs.disableTypeChecked,
  },
  {
    files: ['app/**/*.ts', 'app/**/*.tsx', 'tests/**/*.ts', 'tests/**/*.tsx', '*.config.ts'],
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      react,
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
      'jsx-a11y': jsxA11y,
    },
    settings: {
      react: { version: 'detect' },
    },
    rules: {
      ...react.configs.recommended.rules,
      ...react.configs['jsx-runtime'].rules,
      ...reactHooks.configs.flat.recommended.rules,
      ...jsxA11y.flatConfigs.recommended.rules,
      '@typescript-eslint/consistent-type-imports': ['error', { prefer: 'type-imports' }],
      '@typescript-eslint/no-explicit-any': 'error',
      '@typescript-eslint/no-confusing-void-expression': 'off',
      '@typescript-eslint/no-misused-promises': [
        'error',
        { checksVoidReturn: { arguments: false, attributes: false } },
      ],
      'no-restricted-imports': ['error', { patterns: restrictedReferencePatterns }],
      'react/prop-types': 'off',
      'react-refresh/only-export-components': ['warn', { allowConstantExport: true }],
    },
  },
  {
    files: ['app/src/renderer/**/*.{ts,tsx}', 'app/src/shared/**/*.{ts,tsx}'],
    languageOptions: { globals: globals.browser },
    rules: {
      'no-restricted-imports': [
        'error',
        {
          paths: [
            { name: 'electron', message: 'Renderer/shared code cannot import Electron.' },
            { name: 'node:fs', message: 'Renderer/shared code cannot import Node.' },
            { name: 'node:path', message: 'Renderer/shared code cannot import Node.' },
          ],
          patterns: [
            ...restrictedReferencePatterns,
            { group: ['node:*'], message: 'Renderer/shared code cannot import Node.' },
            {
              group: ['**/main/**', '**/preload/**'],
              message: 'Renderer/shared boundary violation.',
            },
          ],
        },
      ],
    },
  },
  {
    files: ['app/src/main/**/*.ts'],
    languageOptions: { globals: globals.node },
    rules: {
      'no-restricted-imports': [
        'error',
        {
          paths: [
            {
              name: 'electron',
              importNames: ['ipcMain'],
              message: 'Raw ipcMain is restricted to main/ipc/transport.ts.',
            },
          ],
          patterns: restrictedReferencePatterns,
        },
      ],
    },
  },
  {
    files: ['app/src/main/ipc/transport.ts'],
    rules: {
      'no-restricted-imports': ['error', { patterns: restrictedReferencePatterns }],
      '@typescript-eslint/no-unnecessary-type-parameters': 'off',
    },
  },
  {
    files: ['app/src/shared/ipc/registry.ts'],
    rules: {
      '@typescript-eslint/no-unnecessary-type-parameters': 'off',
    },
  },
  {
    files: ['app/src/preload/**/*.ts'],
    languageOptions: { globals: globals.node },
    rules: {
      'no-restricted-imports': [
        'error',
        {
          paths: [
            {
              name: 'electron',
              importNames: ['ipcRenderer'],
              message: 'Raw ipcRenderer is restricted to preload/transport.ts.',
            },
          ],
          patterns: [
            ...restrictedReferencePatterns,
            { group: ['**/main/**', '**/renderer/**'], message: 'Preload boundary violation.' },
          ],
        },
      ],
    },
  },
  {
    files: ['app/src/preload/transport.ts'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          patterns: [
            ...restrictedReferencePatterns,
            { group: ['**/main/**', '**/renderer/**'], message: 'Preload boundary violation.' },
          ],
        },
      ],
    },
  },
  {
    files: ['*.config.{ts,mjs}', 'scripts/**/*.mjs', 'tests/**/*.mjs', 'eslint.config.mjs'],
    languageOptions: { globals: globals.node },
  },
  {
    files: ['build/**/*.cjs', 'app/**/*.cjs', 'tests/**/*.cjs'],
    languageOptions: { globals: globals.node },
    rules: {
      '@typescript-eslint/no-require-imports': 'off',
    },
  },
  {
    files: ['tests/**/*.{ts,tsx}'],
    languageOptions: {
      globals: { ...globals.node, ...globals.browser },
    },
  },
);
