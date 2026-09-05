import {
  helperParamsSchemas,
  type ActivationBinding,
  type HelperParams,
  type HelperResult,
} from '../../shared/helper/protocol';
import { deepFreezeShortcut, shortcutsEqual } from '../../shared/schemas/shortcut';

export interface ActivationConfiguration {
  readonly enabled: boolean;
  readonly bindings: readonly ActivationBinding[];
}

export function createActivationConfiguration(
  enabled: boolean,
  bindings: readonly ActivationBinding[],
): ActivationConfiguration {
  const configured = helperParamsSchemas['activation.configure'].parse({
    enabled,
    bindings: [...bindings],
  });
  return Object.freeze({
    enabled: configured.enabled,
    bindings: Object.freeze(
      configured.bindings.map((binding) =>
        Object.freeze({
          profileId: binding.profileId,
          shortcut: deepFreezeShortcut(binding.shortcut),
        }),
      ),
    ),
  });
}

export function activationAcknowledgementMatches(
  requested: HelperParams<'activation.configure'>,
  effective: HelperResult<'activation.configure'>,
): boolean {
  if (effective.enabled !== requested.enabled) return false;
  if (effective.bindings.length !== requested.bindings.length) return false;
  return effective.bindings.every((binding, index) => {
    const candidate = requested.bindings[index];
    return (
      binding.profileId === candidate?.profileId &&
      shortcutsEqual(binding.shortcut, candidate.shortcut)
    );
  });
}
