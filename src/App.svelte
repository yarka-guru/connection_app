<script>
import { onMount, onDestroy } from 'svelte'
import { safeTimeout, autoFocus } from './lib/utils.js'
import { applyTheme, themes, resolveTheme } from './lib/themes.js'
import SavedConnections from './lib/SavedConnections.svelte'
import ConnectionForm from './lib/ConnectionForm.svelte'
import SessionStatus from './lib/SessionStatus.svelte'
import UpdateBanner from './lib/UpdateBanner.svelte'
import Settings from './lib/Settings.svelte'
import ConfirmDialog from './lib/ConfirmDialog.svelte'

let projects = $state([])
let profiles = $state([])
let selectedProject = $state('')
let selectedProfile = $state('')
let selectedDatabase = $state('')
let connectionStatus = $state('disconnected')
let statusMessage = $state('')
let errorMessage = $state('')
let ready = $state(false)
let initStatus = $state('Initializing...')
let initFailed = $state(false)
let loadingProjects = $state(false)

// New state for features
let savedConnections = $state([])
let activeConnections = $state([])
let updateInfo = $state(null)
let showUpdateBanner = $state(true)
let currentVersion = $state('')
let connectingId = $state(null) // Track which saved connection is being connected
let showSavePrompt = $state(false)
let lastConnectedConfig = $state(null)
let saveConnectionName = $state('')
let showDeleteConfirm = $state(null)
let showCloseConfirm = $state(false)
let isCheckingUpdates = $state(false)
let updateCheckMessage = $state('')
let updateProgress = $state(null)
let updateError = $state(null)

let scheme = $state('dark')
let activeTab = $state('rds')
let currentTheme = $state('forest')
let showSettings = $state(false)
let showSetupScreen = $state(false)
let setupError = $state('')
let isGrantingAccess = $state(false)
let showMigrationOffer = $state(false)
let isImporting = $state(false)
let migrationResult = $state('')

let invoke = null
let listen = null
let appWindow = null

// Cleanup references
let cancelUpdateMsgTimeout = null
let unlistenSsoStatus = null
let unlistenSsoOpenUrl = null
let unlistenStatus = null
let unlistenDisconnected = null
let unlistenConnectionError = null
let unlistenCloseRequested = null
let unlistenUpdateProgress = null
let unlistenConnectionHealth = null
let connectionHealth = $state({})
let systemSchemeCleanup = null
let unlistenTrayQuickConnect = null

// Global keyboard shortcuts
function handleGlobalKeydown(e) {
  // Cmd/Ctrl + , → toggle settings
  if ((e.metaKey || e.ctrlKey) && e.key === ',') {
    e.preventDefault()
    showSettings = !showSettings
  }
  // Ctrl+Q (Linux) / Cmd+Q fallback → quit app
  if ((e.metaKey || e.ctrlKey) && e.key === 'q') {
    e.preventDefault()
    if (activeConnections.length > 0) {
      showCloseConfirm = true
    } else {
      invoke?.('quit_app').catch(() => appWindow?.destroy())
    }
  }
}

onMount(() => {
  window.addEventListener('keydown', handleGlobalKeydown)
  initApp()

  return () => {
    window.removeEventListener('keydown', handleGlobalKeydown)
  }
})

function withTimeout(promise, ms) {
  return Promise.race([
    promise,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error(`Timed out after ${ms / 1000}s`)), ms),
    ),
  ])
}

async function initApp() {
  initFailed = false
  errorMessage = ''

  // Show app immediately — before any async work
  ready = true
  loadingProjects = true

  try {
    // Wait for Tauri IPC bridge before importing modules
    await withTimeout(
      new Promise((resolve) => {
        if (window.__TAURI_INTERNALS__) return resolve()
        const id = setInterval(() => {
          if (window.__TAURI_INTERNALS__) { clearInterval(id); resolve() }
        }, 50)
      }),
      5000,
    )
    const [core, event, windowModule] = await withTimeout(
      Promise.all([
        import('@tauri-apps/api/core'),
        import('@tauri-apps/api/event'),
        import('@tauri-apps/api/window'),
      ]),
      5000,
    )
    invoke = core.invoke
    listen = event.listen
    appWindow = windowModule.getCurrentWindow()
  } catch (err) {
    errorMessage = `Failed to load Tauri API: ${err}`
    loadingProjects = false
    return
  }

  // Load saved theme and scheme
  try {
    const savedTheme = localStorage.getItem('theme')
    const savedScheme = localStorage.getItem('scheme')
    if (savedTheme && themes[savedTheme] && savedTheme !== 'light') {
      currentTheme = savedTheme
    }
    if (savedScheme && ['light', 'dark', 'system'].includes(savedScheme)) {
      scheme = savedScheme
    }
    applyTheme(resolveTheme(scheme, currentTheme))
  } catch (_err) {
    // Non-fatal: use default theme
  }

  // Listen for OS color scheme changes (for "system" mode)
  try {
    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
    const handleSystemChange = () => {
      if (scheme === 'system') {
        applyTheme(resolveTheme('system', currentTheme))
      }
    }
    mediaQuery.addEventListener('change', handleSystemChange)
    systemSchemeCleanup = () => mediaQuery.removeEventListener('change', handleSystemChange)
  } catch (_err) {
    // Non-fatal: system scheme detection not available
  }

  // Check sandbox status — if sandboxed and no AWS access, show setup screen
  try {
    const sandboxStatus = await invoke('get_sandbox_status')
    if (sandboxStatus.isSandboxed && !sandboxStatus.hasAwsAccess) {
      showSetupScreen = true
      loadingProjects = false
      return
    }
  } catch (_err) {
    // Non-fatal: if check fails, continue normally (not sandboxed)
  }

  await setupListenersAndLoad()
}

async function setupListenersAndLoad() {
  // Intercept window close — hide to tray instead of quitting
  unlistenCloseRequested = await appWindow.onCloseRequested(async (event) => {
    event.preventDefault()
    await appWindow.hide()
  })

  // Set up named event listeners (direct from Rust backend)
  listen('sso-status', (ev) => {
    statusMessage = ev.payload.message
  }).then((fn) => { unlistenSsoStatus = fn })

  listen('sso-open-url', (ev) => {
    statusMessage = 'Waiting for SSO authorization in browser...'
  }).then((fn) => { unlistenSsoOpenUrl = fn })

  listen('status', (ev) => {
    statusMessage = ev.payload.message
  }).then((fn) => { unlistenStatus = fn })

  listen('disconnected', (ev) => {
    const { connectionId } = ev.payload
    if (connectionId) {
      activeConnections = activeConnections.filter(
        (c) => c.id !== connectionId,
      )
      // Clean up health state for this connection
      const { [connectionId]: _, ...rest } = connectionHealth
      connectionHealth = rest
    }
    if (activeConnections.length === 0) {
      connectionStatus = 'disconnected'
      statusMessage = ''
      connectionHealth = {}
    }
    connectingId = null
  }).then((fn) => { unlistenDisconnected = fn })

  listen('connection-error', (ev) => {
    errorMessage = ev.payload.message
    connectingId = null
  }).then((fn) => { unlistenConnectionError = fn })

  listen('update-progress', (ev) => {
    updateProgress = ev.payload
  }).then((fn) => { unlistenUpdateProgress = fn })

  listen('connection-health', (ev) => {
    const { connectionId, status, lastCheck } = ev.payload
    connectionHealth = { ...connectionHealth, [connectionId]: { status, lastCheck } }
  }).then((fn) => { unlistenConnectionHealth = fn })

  listen('tray-quick-connect', (ev) => {
    const savedId = ev.payload
    const saved = savedConnections.find((c) => c.id === savedId)
    if (saved) {
      handleSavedConnectionConnect(saved)
    }
  }).then((fn) => { unlistenTrayQuickConnect = fn })

  // Load saved data + version with timeout
  try {
    const [savedResult, versionResult] = await withTimeout(
      Promise.all([
        invoke('load_saved_connections'),
        invoke('get_current_version'),
      ]),
      5000,
    )
    savedConnections = savedResult
    currentVersion = versionResult
  } catch (_err) {
    // Non-fatal: app works without saved data
  }

  // Load projects
  try {
    projects = await invoke('list_projects')
  } catch (err) {
    errorMessage = `Failed to load projects: ${err}`
  } finally {
    loadingProjects = false
  }

  // Non-blocking update check
  checkForUpdates()
}

function retryInit() {
  initApp()
}

async function handleGrantAccess() {
  if (isGrantingAccess) return
  isGrantingAccess = true
  setupError = ''

  try {
    await invoke('grant_aws_access')
    // Check if migration is available before continuing
    try {
      const migrationAvailable = await invoke('check_migration_available')
      if (migrationAvailable) {
        showMigrationOffer = true
        isGrantingAccess = false
        return
      }
    } catch (_err) {
      // Non-fatal: skip migration check
    }
    showSetupScreen = false
    await continueAfterSetup()
  } catch (err) {
    setupError = `${err}`
  } finally {
    isGrantingAccess = false
  }
}

async function handleImportProjects() {
  if (isImporting) return
  isImporting = true
  migrationResult = ''

  try {
    const count = await invoke('import_projects_file')
    migrationResult = `Imported ${count} project${count !== 1 ? 's' : ''}`
    await finishSetup()
  } catch (err) {
    if (`${err}`.includes('cancelled')) {
      migrationResult = 'Import cancelled'
    } else {
      migrationResult = `Import failed: ${err}`
    }
  } finally {
    isImporting = false
  }
}

async function skipMigration() {
  await finishSetup()
}

async function finishSetup() {
  showSetupScreen = false
  showMigrationOffer = false
  await continueAfterSetup()
}

async function continueAfterSetup() {
  loadingProjects = true
  await setupListenersAndLoad()
}

onDestroy(() => {
  cancelUpdateMsgTimeout?.()
  unlistenSsoStatus?.()
  unlistenSsoOpenUrl?.()
  unlistenStatus?.()
  unlistenDisconnected?.()
  unlistenConnectionError?.()
  unlistenCloseRequested?.()
  unlistenUpdateProgress?.()
  unlistenConnectionHealth?.()
  systemSchemeCleanup?.()
  unlistenTrayQuickConnect?.()
})

async function confirmClose() {
  showCloseConfirm = false
  try {
    await invoke('disconnect_all')
  } catch (_err) {
    // Best-effort disconnect before closing
  }
  try {
    await invoke('quit_app')
  } catch (_err) {
    // If quit_app fails, force close via window API
    appWindow?.close()
  }
}

function cancelClose() {
  showCloseConfirm = false
}

async function checkForUpdates() {
  if (isCheckingUpdates) return
  isCheckingUpdates = true
  updateCheckMessage = ''
  try {
    updateInfo = await invoke('check_for_updates')
    if (updateInfo?.updateAvailable) {
      showUpdateBanner = true
    } else {
      updateCheckMessage = 'You are up to date!'
      cancelUpdateMsgTimeout?.()
      cancelUpdateMsgTimeout = safeTimeout(() => {
        updateCheckMessage = ''
      }, 3000)
    }
  } catch (_err) {
    updateCheckMessage = 'Could not check for updates'
    cancelUpdateMsgTimeout?.()
    cancelUpdateMsgTimeout = safeTimeout(() => {
      updateCheckMessage = ''
    }, 3000)
  } finally {
    isCheckingUpdates = false
  }
}

async function loadProfiles() {
  if (!selectedProject) return
  try {
    profiles = await invoke('list_profiles', { projectKey: selectedProject })
    selectedProfile = ''
  } catch (err) {
    errorMessage = `Failed to load profiles: ${err}`
  }
}

async function refreshProjects() {
  try {
    projects = await invoke('list_projects')
  } catch (err) {
    errorMessage = `Failed to refresh projects: ${err}`
  }
}

async function handleConnect() {
  if (!selectedProject || !selectedProfile) return

  errorMessage = ''
  connectionStatus = 'connecting'
  statusMessage = 'Initializing connection...'

  try {
    const result = await invoke('connect', {
      projectKey: selectedProject,
      profile: selectedProfile,
      localPort: null,
      database: selectedDatabase || null,
      savedConnectionId: null,
    })

    // Add to active connections
    activeConnections = [
      ...activeConnections,
      {
        id: result.connectionId,
        projectKey: selectedProject,
        profile: selectedProfile,
        localPort: result.connectionInfo.port,
        connectionInfo: result.connectionInfo,
        status: 'connected',
      },
    ]

    connectionStatus = 'connected'
    statusMessage = 'Tunnel active'

    // Show save prompt
    lastConnectedConfig = {
      projectKey: selectedProject,
      profile: selectedProfile,
      database: selectedDatabase || null,
    }
    showSavePrompt = true
    initSavePrompt()
  } catch (err) {
    errorMessage = `${err}`
    connectionStatus = 'disconnected'
    statusMessage = ''
  }
}

async function handleSavedConnectionConnect(savedConnection) {
  errorMessage = ''
  connectingId = savedConnection.id
  connectionStatus = 'connecting'
  statusMessage = `Connecting to ${savedConnection.name}...`

  try {
    const result = await invoke('connect', {
      projectKey: savedConnection.projectKey,
      profile: savedConnection.profile,
      localPort: null,
      database: savedConnection.database || null,
      savedConnectionId: savedConnection.id,
    })

    // Add to active connections
    activeConnections = [
      ...activeConnections,
      {
        id: result.connectionId,
        savedConnectionId: savedConnection.id,
        projectKey: savedConnection.projectKey,
        profile: savedConnection.profile,
        localPort: result.connectionInfo.port,
        connectionInfo: result.connectionInfo,
        status: 'connected',
      },
    ]

    connectionStatus = 'connected'
    statusMessage = 'Tunnel active'
    connectingId = null
  } catch (err) {
    errorMessage = `${err}`
    connectionStatus =
      activeConnections.length > 0 ? 'connected' : 'disconnected'
    statusMessage = activeConnections.length > 0 ? 'Tunnel active' : ''
    connectingId = null
  }
}

async function handleDisconnect() {
  try {
    await invoke('disconnect_all')
    activeConnections = []
    connectionHealth = {}
    connectionStatus = 'disconnected'
    statusMessage = ''
    showSavePrompt = false
  } catch (err) {
    errorMessage = `Disconnect failed: ${err}`
  }
}

async function handleDisconnectOne(connectionId) {
  try {
    await invoke('disconnect', { connectionId })
    activeConnections = activeConnections.filter((c) => c.id !== connectionId)
    const { [connectionId]: _, ...rest } = connectionHealth
    connectionHealth = rest
    if (activeConnections.length === 0) {
      connectionStatus = 'disconnected'
      statusMessage = ''
      connectionHealth = {}
    }
  } catch (err) {
    errorMessage = `Disconnect failed: ${err}`
  }
}

async function handleDisconnectAll() {
  try {
    await invoke('disconnect_all')
    activeConnections = []
    connectionHealth = {}
    connectionStatus = 'disconnected'
    statusMessage = ''
  } catch (err) {
    errorMessage = `Disconnect all failed: ${err}`
  }
}

function initSavePrompt() {
  if (!lastConnectedConfig) return
  const project = projects.find((p) => p.key === lastConnectedConfig.projectKey)
  saveConnectionName = `${project?.name || lastConnectedConfig.projectKey} - ${lastConnectedConfig.profile}`
}

async function handleSaveConnection() {
  if (!lastConnectedConfig || !saveConnectionName.trim()) return

  try {
    const saved = await invoke('save_connection', {
      name: saveConnectionName.trim(),
      projectKey: lastConnectedConfig.projectKey,
      profile: lastConnectedConfig.profile,
      database: lastConnectedConfig.database || null,
    })
    savedConnections = [
      ...savedConnections.filter((c) => c.id !== saved.id),
      saved,
    ]
    showSavePrompt = false
    saveConnectionName = ''
  } catch (err) {
    errorMessage = `Failed to save connection: ${err}`
  }
}

function handleDeleteSavedConnection(connection) {
  showDeleteConfirm = connection
}

async function confirmDelete() {
  if (!showDeleteConfirm) return
  try {
    await invoke('delete_saved_connection', { id: showDeleteConfirm.id })
    savedConnections = savedConnections.filter(
      (c) => c.id !== showDeleteConfirm.id,
    )
    showDeleteConfirm = null
  } catch (err) {
    errorMessage = `Failed to delete connection: ${err}`
    showDeleteConfirm = null
  }
}

function cancelDelete() {
  showDeleteConfirm = null
}

async function handleUpdateSavedConnection(id, name) {
  try {
    const updated = await invoke('update_saved_connection', { id, name })
    savedConnections = savedConnections.map((c) => (c.id === updated.id ? updated : c))
  } catch (err) {
    errorMessage = `Failed to update connection: ${err}`
  }
}

async function handleReorderSavedConnections(ids) {
  // Optimistic UI update
  const ordered = ids.map((id) => savedConnections.find((c) => c.id === id)).filter(Boolean)
  savedConnections = ordered
  try {
    await invoke('reorder_saved_connections', { ids })
  } catch (err) {
    errorMessage = `Failed to reorder connections: ${err}`
  }
}

async function handleMoveToGroup(id, group) {
  try {
    const updated = await invoke('move_connection_to_group', { id, group })
    savedConnections = savedConnections.map((c) => (c.id === updated.id ? updated : c))
  } catch (err) {
    errorMessage = `Failed to move connection: ${err}`
  }
}

async function handleRenameGroup(oldName, newName) {
  try {
    const updated = await invoke('rename_connection_group', { oldName, newName })
    savedConnections = updated
  } catch (err) {
    errorMessage = `Failed to rename group: ${err}`
  }
}

async function handleDeleteGroup(groupName) {
  try {
    const updated = await invoke('delete_connection_group', { groupName })
    savedConnections = updated
  } catch (err) {
    errorMessage = `Failed to delete group: ${err}`
  }
}

let isUpdating = $state(false)

async function handleInstallUpdate() {
  if (!updateInfo?.updateAvailable || isUpdating) return

  isUpdating = true
  updateError = null
  updateProgress = null

  try {
    await invoke('install_update')
    // App should auto-restart and never reach here, but just in case:
    isUpdating = false
    showUpdateBanner = false
  } catch (err) {
    updateError = `${err}`
    isUpdating = false
    updateProgress = null
  }
}

function handleManualDownload() {
  if (updateInfo?.downloadUrl) {
    invoke?.('open_url', { url: updateInfo.downloadUrl })
  }
}

function handleDismissUpdate() {
  showUpdateBanner = false
  updateError = null
  updateProgress = null
}

function handleProjectChange(newProject) {
  selectedProject = newProject
  selectedDatabase = ''
  loadProfiles()
}

function handleProfileChange(newProfile) {
  selectedProfile = newProfile
}

function handleDatabaseChange(newDatabase) {
  selectedDatabase = newDatabase
}

function dismissError() {
  errorMessage = ''
}

function dismissSavePrompt() {
  showSavePrompt = false
}

function handleThemeChange(themeName) {
  currentTheme = themeName
  applyTheme(resolveTheme(scheme, themeName))
  try {
    localStorage.setItem('theme', themeName)
  } catch (_err) {
    // Non-fatal
  }
}

function handleSchemeChange(newScheme) {
  scheme = newScheme
  applyTheme(resolveTheme(newScheme, currentTheme))
  try {
    localStorage.setItem('scheme', newScheme)
  } catch (_err) {
    // Non-fatal
  }
}

// Filter projects by active tab
const filteredProjects = $derived(
  projects.filter((p) => {
    const ct = p.connectionType || 'rds'
    return activeTab === 'rds' ? ct === 'rds' : ct === 'service'
  }),
)

// Computed: check if the selected project/profile is already saved
const isAlreadySaved = $derived(
  savedConnections.some(
    (c) => c.projectKey === selectedProject && c.profile === selectedProfile && (c.database || null) === (selectedDatabase || null),
  ),
)
</script>

<main>
  {#if !ready}
    <div class="loading-screen">
      <svg width="48" height="48" viewBox="0 0 32 32" fill="none">
        <rect width="32" height="32" rx="8" fill="url(#gradient-loading)"/>
        <path d="M10 12h12M10 16h12M10 20h8" stroke="white" stroke-width="2" stroke-linecap="round"/>
        <circle cx="24" cy="20" r="3" fill="white"/>
        <defs>
          <linearGradient id="gradient-loading" x1="0" y1="0" x2="32" y2="32">
            <stop stop-color="#d4a853"/>
            <stop offset="1" stop-color="#7aab6d"/>
          </linearGradient>
        </defs>
      </svg>
      {#if !initFailed}
        <div class="loading-spinner"></div>
      {/if}
      <span class="loading-text">{initStatus}</span>
      {#if initFailed && errorMessage}
        <p class="init-error-text">{errorMessage}</p>
        <button class="btn-retry" onclick={retryInit}>Retry</button>
      {/if}
    </div>
  {:else if showSetupScreen}
    <div class="setup-screen">
      <div class="setup-card">
        <svg width="48" height="48" viewBox="0 0 32 32" fill="none">
          <rect width="32" height="32" rx="8" fill="url(#gradient-setup)"/>
          <path d="M10 12h12M10 16h12M10 20h8" stroke="white" stroke-width="2" stroke-linecap="round"/>
          <circle cx="24" cy="20" r="3" fill="white"/>
          <defs>
            <linearGradient id="gradient-setup" x1="0" y1="0" x2="32" y2="32">
              <stop stop-color="#d4a853"/>
              <stop offset="1" stop-color="#7aab6d"/>
            </linearGradient>
          </defs>
        </svg>
        {#if showMigrationOffer}
          <h2 class="setup-title">Import Projects</h2>
          <p class="setup-description">
            Would you like to import projects from an existing <code>projects.json</code> file?
          </p>
          <p class="setup-hint">
            If you previously used an older version, you can import your projects from an existing <code>projects.json</code> file.
          </p>
          <div class="setup-actions">
            <button class="btn-grant" onclick={handleImportProjects} disabled={isImporting}>
              {#if isImporting}
                <span class="btn-spinner"></span>
                Importing...
              {:else}
                Import Projects
              {/if}
            </button>
            <button class="btn-skip" onclick={skipMigration} disabled={isImporting}>
              Skip
            </button>
          </div>
          {#if migrationResult}
            <p class={migrationResult.startsWith('Import failed') ? 'setup-error' : 'setup-success'}>{migrationResult}</p>
          {/if}
        {:else}
          <h2 class="setup-title">AWS Directory Access</h2>
          <p class="setup-description">
            ConnectionApp needs access to your <code>~/.aws</code> directory to read AWS profiles and SSO credentials.
          </p>
          <p class="setup-hint">
            You'll be asked to select your <code>~/.aws</code> folder once. Access is remembered for future launches.
          </p>
          <button class="btn-grant" onclick={handleGrantAccess} disabled={isGrantingAccess}>
            {#if isGrantingAccess}
              <span class="btn-spinner"></span>
              Granting access...
            {:else}
              Grant Access
            {/if}
          </button>
          {#if setupError}
            <p class="setup-error">{setupError}</p>
          {/if}
        {/if}
      </div>
    </div>
  {:else}
    <div class="app-container">
      {#if (showUpdateBanner && updateInfo?.updateAvailable) || updateError}
        <UpdateBanner
          {updateInfo}
          {isUpdating}
          {updateProgress}
          {updateError}
          downloadUrl={updateInfo?.downloadUrl}
          installMethod={updateInfo?.installMethod}
          onInstall={handleInstallUpdate}
          onDismiss={handleDismissUpdate}
          onManualDownload={handleManualDownload}
        />
      {/if}

      <header class="app-header">
        <div class="logo">
          <svg width="32" height="32" viewBox="0 0 32 32" fill="none">
            <rect width="32" height="32" rx="8" fill="url(#gradient)"/>
            <path d="M10 12h12M10 16h12M10 20h8" stroke="white" stroke-width="2" stroke-linecap="round"/>
            <circle cx="24" cy="20" r="3" fill="white"/>
            <defs>
              <linearGradient id="gradient" x1="0" y1="0" x2="32" y2="32">
                <stop stop-color="#d4a853"/>
                <stop offset="1" stop-color="#7aab6d"/>
              </linearGradient>
            </defs>
          </svg>
        </div>
        <div class="header-text">
          <h1>ConnectionApp</h1>
          <p>Secure tunneling via AWS SSM</p>
        </div>
      </header>

      {#if activeConnections.length > 0}
        <div class="active-bar">
          <span>{activeConnections.length} active connection{activeConnections.length > 1 ? 's' : ''}</span>
          <button onclick={handleDisconnectAll}>Disconnect All</button>
        </div>
      {/if}

      <div class="tab-bar">
        <button
          class="main-tab"
          class:active={activeTab === 'rds'}
          onclick={() => { activeTab = 'rds'; selectedProject = ''; selectedProfile = ''; profiles = [] }}
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <rect x="2" y="3" width="12" height="10" rx="2" stroke="currentColor" stroke-width="1.5"/>
            <circle cx="5.5" cy="8" r="1" fill="currentColor"/>
            <circle cx="8" cy="8" r="1" fill="currentColor"/>
            <circle cx="10.5" cy="8" r="1" fill="currentColor"/>
          </svg>
          RDS Connect
        </button>
        <button
          class="main-tab"
          class:active={activeTab === 'service'}
          onclick={() => { activeTab = 'service'; selectedProject = ''; selectedProfile = ''; profiles = [] }}
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <rect x="2" y="2" width="12" height="12" rx="2" stroke="currentColor" stroke-width="1.5"/>
            <path d="M5 6h6M5 8h4M5 10h5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
          </svg>
          VNC/RDP Connect
        </button>
      </div>

      <div class="main-content">
        {#if savedConnections.length > 0}
          <SavedConnections
            {savedConnections}
            {activeConnections}
            {projects}
            {connectingId}
            {activeTab}
            {connectionHealth}
            onConnect={handleSavedConnectionConnect}
            onDisconnect={handleDisconnectOne}
            onDelete={handleDeleteSavedConnection}
            onUpdate={handleUpdateSavedConnection}
            onReorder={handleReorderSavedConnections}
            onMoveToGroup={handleMoveToGroup}
            onRenameGroup={handleRenameGroup}
            onDeleteGroup={handleDeleteGroup}
          />
        {/if}


        <ConnectionForm
          projects={filteredProjects}
          {profiles}
          {selectedProject}
          {selectedProfile}
          {selectedDatabase}
          isConnecting={connectionStatus === 'connecting'}
          isLoadingProjects={loadingProjects}
          onProjectChange={handleProjectChange}
          onProfileChange={handleProfileChange}
          onDatabaseChange={handleDatabaseChange}
          onConnect={handleConnect}
        />

        <SessionStatus
          {connectionStatus}
          {statusMessage}
        />

        {#if showSavePrompt && lastConnectedConfig && !isAlreadySaved}
          <div class="save-prompt">
            <span class="save-label">Save this connection:</span>
            <input
              type="text"
              class="save-input"
              bind:value={saveConnectionName}
              placeholder="Connection name"
              onkeydown={(e) => e.key === 'Enter' && handleSaveConnection()}
              use:autoFocus
            />
            <div class="save-prompt-actions">
              <button class="btn-save" onclick={handleSaveConnection}>Save</button>
              <button class="btn-dismiss-save" onclick={dismissSavePrompt}>Cancel</button>
            </div>
          </div>
        {/if}

        {#if errorMessage}
          <div class="error-toast" role="alert">
            <div class="error-icon">
              <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
                <circle cx="10" cy="10" r="9" stroke="currentColor" stroke-width="2"/>
                <path d="M10 6v5M10 13.5v.5" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
              </svg>
            </div>
            <p class="error-text">{errorMessage}</p>
            <button class="dismiss-btn" onclick={dismissError} aria-label="Dismiss error">
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
              </svg>
            </button>
          </div>
        {/if}
      </div>

      <footer class="app-footer">
        <span>v{currentVersion || '0.1.0'}</span>
        {#if updateCheckMessage}
          <span class="update-message">{updateCheckMessage}</span>
        {/if}
        <div class="footer-actions">
          <button class="settings-btn" onclick={() => showSettings = true} aria-label="Open settings">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M6.5 1.5h3l.5 2 1.5.5 1.5-1 2 2-1 1.5.5 1.5 2 .5v3l-2 .5-.5 1.5 1 1.5-2 2-1.5-1-1.5.5-.5 2h-3l-.5-2-1.5-.5-1.5 1-2-2 1-1.5-.5-1.5-2-.5v-3l2-.5.5-1.5-1-1.5 2-2 1.5 1 1.5-.5.5-2z" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
              <circle cx="8" cy="8" r="2" stroke="currentColor" stroke-width="1.2"/>
            </svg>
          </button>
          <button class="check-updates-btn" onclick={checkForUpdates} disabled={isCheckingUpdates}>
            {#if isCheckingUpdates}
              <span class="btn-spinner"></span>
              Checking...
            {:else}
              Check for Updates
            {/if}
          </button>
        </div>
      </footer>
    </div>

    {#if showDeleteConfirm}
      <ConfirmDialog
        title="Delete Connection"
        message='Delete "{showDeleteConfirm.name}"? This action cannot be undone.'
        confirmLabel="Delete"
        cancelLabel="Cancel"
        destructive={true}
        onConfirm={confirmDelete}
        onCancel={cancelDelete}
      />
    {/if}

    {#if showCloseConfirm}
      <ConfirmDialog
        title="Close Application"
        message="All active connections will be closed. Are you sure you want to quit?"
        confirmLabel="Quit"
        cancelLabel="Cancel"
        destructive={true}
        onConfirm={confirmClose}
        onCancel={cancelClose}
      />
    {/if}

    {#if showSettings}
      <Settings
        onClose={() => showSettings = false}
        {invoke}
        onProjectsChanged={refreshProjects}
        {currentTheme}
        onThemeChange={handleThemeChange}
        {scheme}
        onSchemeChange={handleSchemeChange}
      />
    {/if}
  {/if}
</main>

<style>
  :global(:root) {
    --bg-primary: #141e17;
    --bg-secondary: #1a2b1f;
    --bg-tertiary: #182a1d;
    --bg-card: rgba(26, 43, 31, 0.85);
    --bg-card-inner: rgba(28, 40, 30, 0.9);
    --accent-primary: #d4a853;
    --accent-primary-light: #e2c87a;
    --accent-primary-rgb: 212, 168, 83;
    --accent-secondary: #7aab6d;
    --accent-secondary-rgb: 122, 171, 109;
    --text-primary: #d5ddd3;
    --text-secondary: #8a9488;
    --text-muted: #6b7d6a;
    --text-hover: #9baa98;
    --text-inactive: #7d8f7a;
    --color-error: #c9614a;
    --color-error-dark: #b0503c;
    --color-error-soft: #d4836b;
    --color-error-light: #e0a08a;
    --color-error-rgb: 201, 97, 74;
    --color-success: #7aab6d;
    --color-success-soft: #8bbd7a;
    --glass-rgb: 200, 220, 195;
    --glass-bg: rgba(200, 220, 195, 0.04);
    --glass-bg-hover: rgba(200, 220, 195, 0.07);
    --glass-border: rgba(122, 171, 109, 0.08);
    --glass-border-hover: rgba(122, 171, 109, 0.14);
    --glass-blur: blur(16px) saturate(1.8);
    --glass-blur-heavy: blur(32px) saturate(1.8);
    --glass-inner-glow: inset 0 1px 0 rgba(200, 220, 195, 0.06);
    --glass-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
    --bg-button-gradient: linear-gradient(135deg, #d4a853 0%, #7aab6d 100%);
    --bg-button-gradient-shadow: rgba(212, 168, 83, 0.3);
    --press-scale: scale(0.97);
    --transition-fast: 0.15s ease;
    --transition-normal: 0.2s ease;
    --input-bg: rgba(0, 0, 0, 0.3);
    --button-text: white;
    --border-subtle: rgba(255, 255, 255, 0.12);
    --spinner-track: rgba(255, 255, 255, 0.3);
    --spinner-color: white;
    --overlay-bg: rgba(0, 0, 0, 0.8);
    --title-gradient-start: #fff;
  }

  @media (prefers-reduced-transparency) {
    :global(:root) {
      --glass-bg: rgba(26, 43, 31, 0.95);
      --glass-bg-hover: rgba(32, 52, 36, 0.95);
      --glass-blur: none;
      --glass-blur-heavy: none;
    }
  }

  @media (prefers-reduced-motion) {
    :global(:root) {
      --press-scale: none;
      --transition-fast: 0s;
      --transition-normal: 0s;
    }
  }

  :global(*) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Inter', sans-serif;
    background: linear-gradient(145deg, var(--bg-primary) 0%, var(--bg-secondary) 50%, var(--bg-tertiary) 100%);
    min-height: 100vh;
    color: var(--text-primary);
    -webkit-font-smoothing: antialiased;
  }

  main {
    min-height: 100vh;
    padding: 24px;
  }

  .loading-screen {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: calc(100vh - 48px);
    gap: 16px;
  }

  .loading-spinner {
    width: 24px;
    height: 24px;
    border: 2px solid rgba(var(--accent-primary-rgb), 0.2);
    border-top-color: var(--accent-primary);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    will-change: transform;
  }

  .loading-text {
    font-size: 0.875rem;
    color: var(--text-secondary);
  }

  .init-error-text {
    margin: 8px 0 0;
    font-size: 0.8rem;
    color: var(--color-error-light);
    text-align: center;
    max-width: 360px;
    line-height: 1.5;
    word-break: break-word;
  }

  .btn-retry {
    margin-top: 8px;
    padding: 10px 24px;
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--button-text);
    background: var(--bg-button-gradient);
    border: none;
    border-radius: 10px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s;
    box-shadow: 0 4px 12px var(--bg-button-gradient-shadow);
  }

  .btn-retry:hover {
    transform: translateY(-1px);
    box-shadow: 0 6px 16px rgba(var(--accent-primary-rgb), 0.4);
  }

  .btn-retry:active {
    transform: translateY(0);
  }

  .app-container {
    max-width: 480px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .app-header {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 8px 0;
  }

  .logo {
    flex-shrink: 0;
  }

  .header-text h1 {
    margin: 0;
    font-size: 1.5rem;
    font-weight: 600;
    background: linear-gradient(135deg, var(--title-gradient-start) 0%, var(--accent-primary-light) 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
  }

  .header-text p {
    margin: 4px 0 0;
    font-size: 0.875rem;
    color: var(--text-secondary);
  }

  .active-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px;
    border-radius: 10px;
    background: rgba(var(--accent-primary-rgb), 0.1);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.2);
    font-size: 0.8rem;
    color: var(--accent-primary-light);
  }

  .active-bar button {
    padding: 4px 10px;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--text-primary);
    background: rgba(var(--glass-rgb), 0.1);
    border: 1px solid var(--glass-border);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color var(--transition-normal);
  }

  .active-bar button:hover {
    background: rgba(var(--glass-rgb), 0.2);
  }

  .tab-bar {
    display: flex;
    gap: 4px;
    background: rgba(var(--glass-rgb), 0.03);
    padding: 4px;
    border-radius: 14px;
    border: 1px solid var(--glass-border);
  }

  .main-tab {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 12px 16px;
    font-size: 0.9rem;
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: none;
    border-radius: 10px;
    cursor: pointer;
    transition: background-color var(--transition-normal), color var(--transition-normal);
  }

  .main-tab:hover {
    color: var(--text-hover);
    background: var(--glass-bg-hover);
  }

  .main-tab.active {
    background: rgba(var(--accent-primary-rgb), 0.15);
    color: var(--accent-primary-light);
    font-weight: 600;
  }

  .main-content {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .save-prompt {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    background: var(--glass-bg);
    -webkit-backdrop-filter: var(--glass-blur);
    backdrop-filter: var(--glass-blur);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.2);
    border-radius: 12px;
    box-shadow: var(--glass-inner-glow);
    animation: fadeIn 0.3s ease-out;
  }

  .save-label {
    font-size: 0.875rem;
    color: var(--accent-primary);
    font-weight: 500;
  }

  .save-input {
    width: 100%;
    padding: 10px 14px;
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 8px;
    color: var(--text-primary);
    font-size: 0.9rem;
    outline: none;
    transition: border-color 0.2s, box-shadow 0.2s;
  }

  .save-input:focus {
    border-color: var(--accent-primary);
    box-shadow: 0 0 0 2px rgba(var(--accent-primary-rgb), 0.2);
  }

  .save-input::placeholder {
    color: var(--text-secondary);
  }

  .save-prompt-actions {
    display: flex;
    gap: 8px;
  }

  .btn-save {
    padding: 6px 14px;
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--bg-secondary);
    background: var(--accent-primary);
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-save:hover {
    background: var(--accent-primary-light);
  }

  .btn-save:active {
    transform: var(--press-scale);
  }

  .btn-dismiss-save {
    padding: 6px 14px;
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--text-muted);
    background: transparent;
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-dismiss-save:hover {
    background: rgba(var(--glass-rgb), 0.05);
    color: var(--text-hover);
  }

  .error-toast {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 16px;
    background: var(--glass-bg);
    -webkit-backdrop-filter: var(--glass-blur);
    backdrop-filter: var(--glass-blur);
    border: 1px solid rgba(var(--color-error-rgb), 0.2);
    border-radius: 16px;
    box-shadow: var(--glass-inner-glow);
    animation: slideIn 0.3s ease-out;
  }

  @keyframes slideIn {
    from {
      opacity: 0;
      transform: translateY(-8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  @keyframes fadeIn {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .error-icon {
    color: var(--color-error-soft);
    flex-shrink: 0;
    margin-top: 2px;
  }

  .error-text {
    flex: 1;
    margin: 0;
    font-size: 0.875rem;
    color: var(--color-error-light);
    line-height: 1.5;
  }

  .dismiss-btn {
    background: transparent;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
    transition: background-color 0.2s, color 0.2s;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .dismiss-btn:hover {
    background: rgba(var(--glass-rgb), 0.1);
    color: var(--text-hover);
  }

  .app-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding-top: 16px;
  }

  .app-footer > span:first-child {
    font-size: 0.75rem;
    color: var(--text-inactive);
  }

  .footer-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .settings-btn {
    padding: 6px;
    background: transparent;
    border: 1px solid var(--glass-border);
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s, color 0.2s;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .settings-btn:hover {
    background: rgba(var(--glass-rgb), 0.05);
    border-color: var(--border-subtle);
    color: var(--text-hover);
  }

  .settings-btn:active {
    transform: var(--press-scale);
  }

  .check-updates-btn {
    padding: 6px 12px;
    font-size: 0.7rem;
    font-weight: 500;
    color: var(--text-muted);
    background: transparent;
    border: 1px solid var(--glass-border);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s, color 0.2s;
  }

  .check-updates-btn:hover:not(:disabled) {
    background: rgba(var(--glass-rgb), 0.05);
    border-color: var(--border-subtle);
    color: var(--text-hover);
  }

  .check-updates-btn:active:not(:disabled) {
    transform: var(--press-scale);
  }

  .check-updates-btn:disabled {
    opacity: 0.7;
    cursor: wait;
  }

  .check-updates-btn .btn-spinner {
    display: inline-block;
    width: 10px;
    height: 10px;
    border: 1.5px solid var(--spinner-track);
    border-top-color: var(--text-hover);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    margin-right: 4px;
  }

  .update-message {
    font-size: 0.75rem;
    color: var(--accent-secondary);
    animation: fadeIn 0.3s ease-out;
  }

  .setup-screen {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: calc(100vh - 48px);
  }

  .setup-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
    max-width: 400px;
    padding: 32px;
    background: var(--glass-bg);
    -webkit-backdrop-filter: var(--glass-blur);
    backdrop-filter: var(--glass-blur);
    border: 1px solid var(--glass-border);
    border-radius: 20px;
    box-shadow: var(--glass-shadow), var(--glass-inner-glow);
    text-align: center;
    animation: fadeIn 0.3s ease-out;
  }

  .setup-title {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 600;
    color: var(--text-primary);
  }

  .setup-description {
    margin: 0;
    font-size: 0.9rem;
    color: var(--text-secondary);
    line-height: 1.5;
  }

  .setup-description code,
  .setup-hint code {
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.85em;
    color: var(--accent-primary-light);
    background: rgba(var(--accent-primary-rgb), 0.15);
    padding: 1px 5px;
    border-radius: 4px;
  }

  .setup-hint {
    margin: 0;
    font-size: 0.8rem;
    color: var(--text-muted);
    line-height: 1.5;
  }

  .btn-grant {
    margin-top: 8px;
    padding: 12px 32px;
    font-size: 0.9rem;
    font-weight: 600;
    color: white;
    background: var(--bg-button-gradient);
    border: none;
    border-radius: 12px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s;
    box-shadow: 0 4px 16px var(--bg-button-gradient-shadow);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .btn-grant:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 6px 20px rgba(var(--accent-primary-rgb), 0.4);
  }

  .btn-grant:active:not(:disabled) {
    transform: translateY(0);
  }

  .btn-grant:disabled {
    opacity: 0.7;
    cursor: wait;
  }

  .btn-grant .btn-spinner {
    display: inline-block;
    width: 14px;
    height: 14px;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: white;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .setup-error {
    margin: 0;
    font-size: 0.8rem;
    color: var(--color-error-light);
    line-height: 1.5;
    max-width: 100%;
    word-break: break-word;
  }

  .setup-success {
    margin: 0;
    font-size: 0.8rem;
    color: var(--accent-secondary);
    line-height: 1.5;
  }

  .setup-actions {
    display: flex;
    gap: 12px;
    align-items: center;
    margin-top: 8px;
  }

  .btn-skip {
    padding: 12px 24px;
    font-size: 0.9rem;
    font-weight: 500;
    color: var(--text-muted);
    background: transparent;
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 12px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-skip:hover:not(:disabled) {
    background: rgba(var(--glass-rgb), 0.05);
    color: var(--text-hover);
  }

  .btn-skip:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
