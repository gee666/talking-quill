import { readFileSync } from 'node:fs';

/** Read the composition modules in startup order for source-policy assertions. */
export function readApplicationSource(): string {
  return [
    'application',
    'application-startup',
    'application-startup-foundation',
    'application-startup-services',
    'application-startup-interaction',
    'application-startup-completion',
    'application-helper',
    'application-local-update',
  ]
    .map((name) => readFileSync(`app/src/main/app/${name}.ts`, 'utf8'))
    .join('\n');
}
