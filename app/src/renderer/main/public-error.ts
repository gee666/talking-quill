const MAX_PUBLIC_ERROR_LENGTH = 240;

export function publicErrorMessage(error: unknown, fallback: string): string {
  if (
    error instanceof Error &&
    error.message.length > 0 &&
    error.message.length <= MAX_PUBLIC_ERROR_LENGTH
  ) {
    return error.message;
  }
  return fallback;
}
