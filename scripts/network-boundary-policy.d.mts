export interface ApprovedNetworkBoundary {
  readonly category: string;
  readonly reason: string;
  readonly tokens: readonly string[];
}

export const APPROVED_NETWORK_BOUNDARIES: Readonly<Record<string, ApprovedNetworkBoundary>>;
export function detectNetworkTokens(source: string, fileName?: string): readonly string[];
export function verifyNetworkBoundary(root?: string): Promise<
  readonly {
    readonly path: string;
    readonly category: string;
    readonly reason: string;
  }[]
>;
