export interface NativePublicationEntry {
  readonly name: string;
  readonly path: string;
  readonly bytes: number;
  readonly sha256: string;
  readonly identity: string;
}

export interface NativePublicationDescriptor {
  readonly schemaVersion: 1;
  readonly purpose: string;
  readonly buildId: string;
  readonly layout: 'producer' | 'bundle';
  readonly removeNativeBase: boolean;
  readonly nativeBase: string;
  readonly nativeRoot: string;
  readonly userSid: string;
  readonly rootIdentity: string;
  readonly inventory: readonly NativePublicationEntry[];
  readonly cleanupLauncher: {
    readonly path: string;
    readonly bytes: number;
    readonly sha256: string;
    readonly identity: string;
  };
  readonly descriptorPath: string;
}

export function publishAcceptanceNative(options: {
  buildId: string;
  sourceRoot: string;
  outputRoot: string;
  programData?: string;
  layout?: 'producer' | 'bundle';
}): Promise<Readonly<NativePublicationDescriptor>>;
export function cleanupAcceptanceNative(
  descriptor: NativePublicationDescriptor,
  options?: { programData?: string },
): Promise<Readonly<{ result: string }>>;
export function cleanupAcceptanceNativeDescriptor(
  descriptorPath: string,
  options?: { programData?: string },
): Promise<Readonly<{ result: string }>>;
export function validateDescriptor(value: unknown): NativePublicationDescriptor;
