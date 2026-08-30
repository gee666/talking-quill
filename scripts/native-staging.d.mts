export function replaceNativeRoleDirectory(options: {
  readonly appDirectory: string;
  readonly stagingDirectory: string;
  readonly platform: 'win32' | 'darwin';
  readonly architecture: 'x64' | 'arm64';
  readonly pid?: number;
  readonly processAlive?: (pid: number) => boolean;
}): Promise<void>;
