// The production installer has a fixed Program Files destination. NSIS ignores
// /D for this package, so an arbitrary "isolated root" cannot isolate machine
// lifecycle mutation. Keep this retired entry point fail-closed rather than
// risk redirecting only the test profile while mutating the real installation.
throw new Error(
  'nsis-upgrade-evidence is retired: use windows-installed-acceptance on a disposable Windows host',
);
