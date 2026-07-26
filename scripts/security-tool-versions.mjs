export const CARGO_AUDIT_VERSION = '0.22.2';

export function isExpectedCargoAuditVersion(output) {
  // `cargo audit --version` reports the cargo subcommand package name with the
  // subcommand appended by Cargo (cargo-audit-audit), not the binary filename.
  return output.trim() === `cargo-audit-audit ${CARGO_AUDIT_VERSION}`;
}
