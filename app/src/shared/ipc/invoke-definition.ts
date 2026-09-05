import { z } from 'zod';
import type { WindowRole } from '../constants/app';

export const emptyRequest = z.object({}).strict();
export const acknowledgement = z.object({ accepted: z.literal(true) }).strict();
export const defineInvoke = <
  const Roles extends readonly WindowRole[],
  Request extends z.ZodType,
  Response extends z.ZodType,
>(definition: {
  readonly roles: Roles;
  readonly request: Request;
  readonly response: Response;
}): Readonly<{ readonly roles: Roles; readonly request: Request; readonly response: Response }> =>
  Object.freeze(definition);
