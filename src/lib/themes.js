/**
 * Theme registry.
 *
 * Tokens live in src/app.css under :root (default theme = Aubergine) and
 * [data-theme-variant="..."] / [data-theme="..."] selectors. This module
 * only (1) toggles the HTML attributes that select a theme and (2) holds
 * a small palette sample so Settings can render preview swatches without
 * pulling all CSS.
 */

// key → preview palette (subset of tokens used by the theme-preview card in Settings).
const previews = {
  aubergine: {
    name: 'Aubergine',
    variant: null,
    mode: 'dark',
    '--bg-primary': '#14091a',
    '--accent-primary': '#e4895c',
    '--accent-secondary': '#c4a7e7',
    '--text-primary': '#f5ecef',
    '--text-secondary': '#b9a6b8',
    '--bg-button-gradient':
      'linear-gradient(135deg, #e4895c 0%, #f5ad7a 50%, #c4a7e7 100%)',
  },
  'obsidian-classic': {
    name: 'Obsidian',
    variant: 'obsidian-classic',
    mode: 'dark',
    '--bg-primary': '#080808',
    '--accent-primary': '#10b981',
    '--accent-secondary': '#34d399',
    '--text-primary': '#e5e5e5',
    '--text-secondary': '#9e9e9e',
    '--bg-button-gradient': 'linear-gradient(135deg, #10b981 0%, #34d399 100%)',
  },
  midnight: {
    name: 'Midnight',
    variant: 'midnight',
    mode: 'dark',
    '--bg-primary': '#070a14',
    '--accent-primary': '#6b9fff',
    '--accent-secondary': '#b08bff',
    '--text-primary': '#e2e8f0',
    '--text-secondary': '#94a3b8',
    '--bg-button-gradient': 'linear-gradient(135deg, #6b9fff 0%, #b08bff 100%)',
  },
  ember: {
    name: 'Ember',
    variant: 'ember',
    mode: 'dark',
    '--bg-primary': '#110704',
    '--accent-primary': '#f59e0b',
    '--accent-secondary': '#ef6a6a',
    '--text-primary': '#f5ebe0',
    '--text-secondary': '#a89888',
    '--bg-button-gradient': 'linear-gradient(135deg, #f59e0b 0%, #ef4444 100%)',
  },
  arctic: {
    name: 'Arctic',
    variant: 'arctic',
    mode: 'dark',
    '--bg-primary': '#061014',
    '--accent-primary': '#22d3ee',
    '--accent-secondary': '#7dd3fc',
    '--text-primary': '#e2e8f0',
    '--text-secondary': '#94a3b8',
    '--bg-button-gradient': 'linear-gradient(135deg, #22d3ee 0%, #7dd3fc 100%)',
  },
  rosewood: {
    name: 'Rosewood',
    variant: 'rosewood',
    mode: 'dark',
    '--bg-primary': '#140610',
    '--accent-primary': '#fb7185',
    '--accent-secondary': '#d8b4fe',
    '--text-primary': '#f0e4f0',
    '--text-secondary': '#a890a8',
    '--bg-button-gradient': 'linear-gradient(135deg, #fb7185 0%, #d8b4fe 100%)',
  },
  light: {
    name: 'Light',
    variant: null,
    mode: 'light',
    '--bg-primary': '#fbf6f3',
    '--accent-primary': '#c16c3f',
    '--accent-secondary': '#8b5cf6',
    '--text-primary': '#2a1a20',
    '--text-secondary': '#6b4f5a',
    '--bg-button-gradient':
      'linear-gradient(135deg, #c16c3f 0%, #e4895c 60%, #c4a7e7 100%)',
  },
}

// Keep the legacy vars shape so existing preview cards keep working.
export const themes = Object.fromEntries(
  Object.entries(previews).map(([key, p]) => [
    key,
    {
      key,
      name: p.name,
      variant: p.variant,
      mode: p.mode,
      vars: {
        '--bg-primary': p['--bg-primary'],
        '--accent-primary': p['--accent-primary'],
        '--accent-secondary': p['--accent-secondary'],
        '--text-primary': p['--text-primary'],
        '--text-secondary': p['--text-secondary'],
        '--bg-button-gradient': p['--bg-button-gradient'],
      },
    },
  ]),
)

export const darkThemeNames = [
  'aubergine',
  'obsidian-classic',
  'midnight',
  'ember',
  'arctic',
  'rosewood',
]
export const lightThemeNames = ['light']

export const schemeModes = ['light', 'dark', 'system']

// Migrate legacy theme keys saved in localStorage by pre-Aubergine builds.
const LEGACY_MAP = {
  forest: 'obsidian-classic',
  cream: 'light',
  frost: 'light',
}

export function migrateThemeKey(key) {
  if (!key) return key
  return LEGACY_MAP[key] ?? key
}

/**
 * Apply a theme by toggling `data-theme` / `data-theme-variant` on <html>.
 * All CSS tokens are scoped to those selectors in app.css.
 *
 * @param {string} name theme key (e.g. 'aubergine', 'light')
 */
export function applyTheme(name) {
  const key = migrateThemeKey(name)
  const theme = themes[key] || themes.aubergine
  const root = document.documentElement

  if (theme.mode === 'light') {
    root.setAttribute('data-theme', 'light')
  } else {
    root.setAttribute('data-theme', 'dark')
  }

  if (theme.variant) {
    root.setAttribute('data-theme-variant', theme.variant)
  } else {
    root.removeAttribute('data-theme-variant')
  }
}

/**
 * Resolve which theme key to apply for the current scheme.
 *
 * @param {'light'|'dark'|'system'} scheme
 * @param {string} darkTheme dark theme key (e.g. 'aubergine')
 * @param {string} [lightTheme='light'] light theme key
 * @returns {string}
 */
export function resolveTheme(scheme, darkTheme, lightTheme = 'light') {
  const dark = migrateThemeKey(darkTheme) || 'aubergine'
  const light = migrateThemeKey(lightTheme) || 'light'
  if (scheme === 'light') return light
  if (scheme === 'dark') return dark
  if (
    typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-color-scheme: light)').matches
  ) {
    return light
  }
  return dark
}

/** @returns {'light'|'dark'} current OS scheme preference */
export function getSystemScheme() {
  if (
    typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-color-scheme: light)').matches
  ) {
    return 'light'
  }
  return 'dark'
}
