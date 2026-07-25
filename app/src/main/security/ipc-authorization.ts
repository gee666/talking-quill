import type { WindowRole } from '../../shared/constants/app';
import { PublicAppError } from './public-error';

export interface IpcAuthorizationInput {
  readonly registeredRole: WindowRole | null;
  readonly allowedRoles: readonly WindowRole[];
  readonly isMainFrame: boolean;
  readonly frameUrl: string;
  readonly expectedUrl: string | null;
}

export function authorizeIpc(input: IpcAuthorizationInput): WindowRole {
  if (
    input.registeredRole === null ||
    !input.isMainFrame ||
    input.expectedUrl === null ||
    input.frameUrl !== input.expectedUrl ||
    !input.allowedRoles.some((role) => role === input.registeredRole)
  ) {
    throw new PublicAppError({
      code: 'FORBIDDEN',
      message: 'This window is not authorized.',
    });
  }
  return input.registeredRole;
}
