export function parseTcpdumpInterfaces(source: string): readonly string[];
export function selectMacCaptureInterface(
  interfaces: readonly string[],
  requested?: string,
  scope?: string,
): string;
export function tcpdumpArguments(
  platform: string,
  output: string,
  interfaces: readonly string[],
  requested: string,
  scope: string,
): readonly string[];
export interface CaptureStatus {
  readonly schemaVersion: 2;
  readonly evidenceType: 'os-packet-capture';
  readonly platform: string;
  readonly tool: string | null;
  readonly toolAvailable: boolean;
  readonly interfaces: readonly string[];
  readonly permissionVerified: false;
  readonly limitation: string;
}
export function detectCaptureStatus(platform?: string): CaptureStatus;
