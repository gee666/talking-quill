export interface EnvironmentControl {
  readonly protection_rules?: readonly {
    readonly type?: string;
    readonly reviewers?: readonly unknown[];
    readonly prevent_self_review?: boolean;
  }[];
  readonly deployment_branch_policy?: {
    readonly protected_branches?: boolean;
    readonly custom_branch_policies?: boolean;
  };
}

export interface ExternalControls {
  readonly repository: {
    readonly full_name?: string;
    readonly default_branch?: string;
    readonly archived?: boolean;
    readonly disabled?: boolean;
    readonly visibility?: string;
    readonly private?: boolean;
    readonly immutable_releases?: boolean;
    readonly defaultBranchProtected?: boolean;
  };
  readonly environments: Readonly<Record<string, EnvironmentControl>>;
  readonly tagRulesets: readonly {
    readonly target?: string;
    readonly enforcement?: string;
    readonly rules?: readonly { readonly type?: string }[];
  }[];
  readonly runnerLabels: readonly string[];
  readonly environmentSecrets: Readonly<Record<string, readonly string[]>>;
  readonly environmentVariables: Readonly<Record<string, readonly string[]>>;
}

export interface ExternalControlPolicy {
  readonly repository: string;
  readonly defaultBranch: string;
  readonly environments: readonly string[];
  readonly runnerLabels: readonly string[];
  readonly environmentSecrets: Readonly<Record<string, readonly string[]>>;
  readonly environmentVariables: Readonly<Record<string, readonly string[]>>;
}

export function validateExternalControls(
  controls: ExternalControls,
  policy: ExternalControlPolicy,
): void;
