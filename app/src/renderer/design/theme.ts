import { useCallback, useEffect, useState } from 'react';

export type ThemeName = 'light' | 'dark';

const STORAGE_KEY = 'talking-quill.theme';

function isTheme(value: unknown): value is ThemeName {
  return value === 'light' || value === 'dark';
}

export function readStoredTheme(): ThemeName | null {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    return isTheme(stored) ? stored : null;
  } catch {
    return null;
  }
}

export function resolveInitialTheme(): ThemeName {
  const stored = readStoredTheme();
  if (stored !== null) return stored;
  try {
    return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  } catch {
    return 'dark';
  }
}

export function applyTheme(theme: ThemeName): void {
  document.documentElement.dataset.theme = theme;
}

function storeTheme(theme: ThemeName): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // A restricted storage partition must never break rendering.
  }
}

/** Applies the resolved theme and keeps every renderer window in sync. */
export function useTheme(): readonly [ThemeName, (next: ThemeName) => void] {
  const [theme, setTheme] = useState<ThemeName>(resolveInitialTheme);
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== STORAGE_KEY) return;
      if (isTheme(event.newValue)) setTheme(event.newValue);
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);
  const select = useCallback((next: ThemeName) => {
    storeTheme(next);
    setTheme(next);
  }, []);
  return [theme, select] as const;
}
