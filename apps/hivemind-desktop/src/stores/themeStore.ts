import { createSignal } from 'solid-js';

export type ThemeName = 'dark' | 'light' | 'midnight-blue' | 'catppuccin-mocha' | 'catppuccin-latte' | 'borg' | 'lcars';

export interface ThemeDefinition {
  name: ThemeName;
  label: string;
  family: 'dark' | 'light';
}

export const availableThemes: ThemeDefinition[] = [
  { name: 'dark', label: 'Dark', family: 'dark' },
  { name: 'light', label: 'Light', family: 'light' },
  { name: 'midnight-blue', label: 'Midnight Blue', family: 'dark' },
  { name: 'catppuccin-mocha', label: 'Catppuccin Mocha', family: 'dark' },
  { name: 'catppuccin-latte', label: 'Catppuccin Latte', family: 'light' },
  { name: 'borg', label: 'Borg', family: 'dark' },
  { name: 'lcars', label: 'LCARS', family: 'dark' },
];

const STORAGE_KEY = 'hivemind-theme';

function detectSystemTheme(): ThemeName {
  if (typeof window !== 'undefined' && window.matchMedia?.('(prefers-color-scheme: light)').matches) {
    return 'light';
  }
  return 'dark';
}

function loadSavedTheme(): ThemeName {
  if (typeof window === 'undefined') return 'dark';
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved && availableThemes.some((t) => t.name === saved)) {
    return saved as ThemeName;
  }
  return detectSystemTheme();
}

function applyThemeToDOM(theme: ThemeName) {
  if (typeof document === 'undefined') return;
  const html = document.documentElement;
  // The default (dark) theme uses :root variables, so we remove the attribute
  if (theme === 'dark') {
    html.removeAttribute('data-theme');
  } else {
    html.setAttribute('data-theme', theme);
  }
}

const initial = loadSavedTheme();
applyThemeToDOM(initial);

// Module-scope signal — safe for Tauri desktop (no SSR). Provides global singleton theme state.
const [currentTheme, setCurrentThemeSignal] = createSignal<ThemeName>(initial);

export function setTheme(theme: ThemeName) {
  setCurrentThemeSignal(theme);
  if (typeof window !== 'undefined') {
    localStorage.setItem(STORAGE_KEY, theme);
  }
  applyThemeToDOM(theme);
}

export function getThemeFamily(): 'dark' | 'light' {
  const theme = currentTheme();
  return availableThemes.find((t) => t.name === theme)?.family ?? 'dark';
}

// Reactively track OS theme changes when user hasn't chosen a specific theme
if (typeof window !== 'undefined' && window.matchMedia) {
  const mql = window.matchMedia('(prefers-color-scheme: dark)');
  mql.addEventListener('change', () => {
    const saved = localStorage.getItem(STORAGE_KEY);
    // Only react to OS changes if the user hasn't explicitly chosen a theme
    if (!saved || !availableThemes.some((t) => t.name === saved)) {
      const systemTheme = detectSystemTheme();
      setCurrentThemeSignal(systemTheme);
      applyThemeToDOM(systemTheme);
    }
  });
}

export { currentTheme };
