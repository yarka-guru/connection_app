/**
 * Copy a value to the system clipboard.
 * @param {string} value
 * @returns {Promise<boolean>}
 */
export async function copyToClipboard(value) {
  try {
    await navigator.clipboard.writeText(value)
    return true
  } catch {
    return false
  }
}

/**
 * Mask a password string with asterisks.
 * @param {string} password
 * @returns {string}
 */
export function maskPassword(password) {
  if (!password) return ''
  return '*'.repeat(Math.min(password.length, 20))
}

/**
 * setTimeout wrapper that returns a cancel function.
 * @param {Function} callback
 * @param {number} delay
 * @returns {Function} cancel
 */
export function safeTimeout(callback, delay) {
  const id = setTimeout(callback, delay)
  return () => clearTimeout(id)
}

/**
 * Svelte action: traps focus within a DOM node (for modals).
 * Caches focusable elements and refreshes on DOM mutations.
 * @param {HTMLElement} node
 */
export function trapFocus(node) {
  const focusableSelector =
    'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'

  let focusableElements = [...node.querySelectorAll(focusableSelector)]

  // Re-query when children change (e.g. conditional content)
  const observer = new MutationObserver(() => {
    focusableElements = [...node.querySelectorAll(focusableSelector)]
  })
  observer.observe(node, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ['disabled', 'tabindex'],
  })

  function handleKeydown(e) {
    if (e.key !== 'Tab') return
    if (focusableElements.length === 0) return

    const first = focusableElements[0]
    const last = focusableElements[focusableElements.length - 1]

    if (e.shiftKey) {
      if (document.activeElement === first) {
        e.preventDefault()
        last.focus()
      }
    } else {
      if (document.activeElement === last) {
        e.preventDefault()
        first.focus()
      }
    }
  }

  node.addEventListener('keydown', handleKeydown)

  // Focus the first focusable element on mount
  if (focusableElements.length > 0) {
    focusableElements[0].focus()
  }

  return {
    destroy() {
      node.removeEventListener('keydown', handleKeydown)
      observer.disconnect()
    },
  }
}

/**
 * Svelte action: auto-focuses the node on mount.
 * @param {HTMLElement} node
 */
export function autoFocus(node) {
  node.focus()
}

/**
 * Escape a string for safe use inside single quotes in a shell command.
 * Replaces each single quote with the sequence: end quote, escaped quote, start quote.
 * @param {string} str
 * @returns {string}
 */
function shellEscape(str) {
  return str.replace(/'/g, "'\\''")
}

/**
 * Build a connection string for a database connection.
 * Shell command formats (psql, mysql) use proper quoting to prevent injection.
 * @param {{ username: string, password: string, database: string, localPort: string|number, engine?: string }} info
 * @param {'psql'|'mysql'|'jdbc'|'uri'} format
 * @returns {string}
 */
export function buildConnectionString(info, format) {
  const host = 'localhost'
  const port = info.localPort

  switch (format) {
    case 'psql':
      return `psql 'host=${shellEscape(host)} port=${shellEscape(String(port))} user=${shellEscape(info.username)} password=${shellEscape(info.password)} dbname=${shellEscape(info.database)}'`
    case 'mysql':
      return `mysql -h ${shellEscape(host)} -P ${shellEscape(String(port))} -u '${shellEscape(info.username)}' -p'${shellEscape(info.password)}' '${shellEscape(info.database)}'`
    case 'jdbc':
      if (info.engine === 'mysql') {
        return `jdbc:mysql://${host}:${port}/${info.database}?user=${encodeURIComponent(info.username)}&password=${encodeURIComponent(info.password)}`
      }
      return `jdbc:postgresql://${host}:${port}/${info.database}?user=${encodeURIComponent(info.username)}&password=${encodeURIComponent(info.password)}`
    case 'uri': {
      const proto = info.engine === 'mysql' ? 'mysql' : 'postgresql'
      return `${proto}://${encodeURIComponent(info.username)}:${encodeURIComponent(info.password)}@${host}:${port}/${info.database}`
    }
    default:
      return ''
  }
}
