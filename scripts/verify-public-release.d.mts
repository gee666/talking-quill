export interface GitHubReleaseIdentity {
  readonly draft?: boolean;
  readonly prerelease?: boolean;
  readonly tag_name?: string;
  readonly target_commitish?: string;
  readonly html_url?: string;
  readonly assets?: readonly { readonly name: string }[];
}

export function validatePublicRelease(
  release: GitHubReleaseIdentity,
  latest: GitHubReleaseIdentity,
  expected: {
    readonly tag: string;
    readonly commit: string;
    readonly assetNames: readonly string[];
  },
): void;
