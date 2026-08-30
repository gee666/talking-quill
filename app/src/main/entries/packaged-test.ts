import { isAbsolute, resolve } from 'node:path';
import { startMain } from '../bootstrap';

const authorized =
  process.env.CI === 'true' &&
  process.env.TALKING_QUILL_PACKAGED_TEST === '1' &&
  process.argv.some((argument) => argument.startsWith('--remote-debugging-port='));
if (!authorized) throw new Error('Packaged test entry requires its explicit test launch contract');

const profile = readArgument('--talking-quill-user-data=');
if (profile === null || !isAbsolute(profile)) {
  throw new Error('Packaged test user-data path must be absolute');
}
const interactiveAppData = process.env.TALKING_QUILL_TEST_INTERACTIVE_APPDATA;
const interactiveHome = process.env.TALKING_QUILL_TEST_INTERACTIVE_HOME;

startMain({
  userDataPath: resolve(profile),
  isolatedInstance: true,
  application: {
    packagedEgressProof: true,
    ...(interactiveAppData === undefined || !isAbsolute(interactiveAppData)
      ? {}
      : { interactiveAppData }),
    ...(interactiveHome === undefined || !isAbsolute(interactiveHome) ? {} : { interactiveHome }),
  },
});

function readArgument(prefix: string): string | null {
  const argument = process.argv.find((value) => value.startsWith(prefix));
  const value = argument?.slice(prefix.length).trim();
  return value === undefined || value.length === 0 ? null : value;
}
