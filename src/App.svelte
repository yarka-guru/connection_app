<script>
import { onMount, onDestroy } from 'svelte'
import { safeTimeout, autoFocus } from './lib/utils.js'
import SavedConnections from './lib/SavedConnections.svelte'
import ActiveConnections from './lib/ActiveConnections.svelte'
import ConnectionForm from './lib/ConnectionForm.svelte'
import SessionStatus from './lib/SessionStatus.svelte'
import UpdateBanner from './lib/UpdateBanner.svelte'
import PrerequisitesCheck from './lib/PrerequisitesCheck.svelte'
import Settings from './lib/Settings.svelte'
import ConfirmDialog from './lib/ConfirmDialog.svelte'

let projects = $state([])
let _profiles = $state([])
let selectedProject = $state('')
let selectedProfile = $state('')
let _connectionStatus = $state('disconnected')
let _statusMessage = $state('')
let _errorMessage = $state('')
let _ready = $state(false)
let _initStatus = $state('Initializing...')
let _initFailed = $state(false)
let _loadingProjects = $state(false)

// New state for features
let savedConnections = $state([])
let activeConnections = $state([])
let updateInfo = $state(null)
let _showUpdateBanner = $state(true)
let _currentVersion = $state('')
let _connectingId = $state(null) // Track which saved connection is being connected
let _showSavePrompt = $state(false)
let lastConnectedConfig = $state(null)
let saveConnectionName = $state('')
let showDeleteConfirm = $state(null)
let isCheckingUpdates = $state(false)
let _updateCheckMessage = $state('')

// Prerequisites and Settings
let _showPrerequisites = $state(false)
let _prerequisitesData = $state([])
let _showSettings = $state(false)

let invoke = null
let listen = null

// Cleanup references
let cancelUpdateMsgTimeout = null
let unlistenSidecar = null

// Global keyboard shortcuts
function handleGlobalKeydown(e) {
  // Cmd/Ctrl + , → toggle settings
  if ((e.metaKey || e.ctrlKey) && e.key === ',') {
    e.preventDefault()
    _showSettings = !_showSettings
  }
}

onMount(() => {
  window.addEventListener('keydown', handleGlobalKeydown)
  _initApp()

  return () => {
    window.removeEventListener('keydown', handleGlobalKeydown)
  }
})

async function _initApp() {
  _initFailed = false
  _errorMessage = ''
  _initStatus = 'Loading modules...'

  try {
    const core = await import('@tauri-apps/api/core')
    const event = await import('@tauri-apps/api/event')
    invoke = core.invoke
    listen = event.listen

    _initStatus = 'Setting up listeners...'
    unlistenSidecar = await listen('sidecar-event', (ev) => {
      const data = ev.payload
      if (data.event === 'status') {
        _statusMessage = data.message
      } else if (data.event === 'credentials') {
        // Credentials are now part of connection info, handled via activeConnections
      } else if (data.event === 'disconnected') {
        const { connectionId } = data
        if (connectionId) {
          activeConnections = activeConnections.filter(
            (c) => c.id !== connectionId,
          )
        }
        if (activeConnections.length === 0) {
          _connectionStatus = 'disconnected'
          _statusMessage = ''
        }
        _connectingId = null
      } else if (data.event === 'error') {
        _errorMessage = data.message
        _connectingId = null
      }
    })

    // Load instant data first (no sidecar dependency)
    _initStatus = 'Loading saved data...'
    const [savedResult, versionResult] = await Promise.all([
      invoke('load_saved_connections'),
      invoke('get_current_version'),
    ])

    savedConnections = savedResult
    _currentVersion = versionResult

    // Show app immediately with saved data
    _ready = true

    // Load projects from sidecar in background (no longer blocks UI)
    _loadingProjects = true
    try {
      projects = await invoke('list_projects')
    } catch (err) {
      _errorMessage = `Failed to load projects: ${err}`
    } finally {
      _loadingProjects = false
    }

    // Non-blocking checks
    checkForUpdates()
    checkPrerequisites()
  } catch (err) {
    _errorMessage = `${err}`
    _initStatus = 'Failed to initialize'
    _initFailed = true
  }
}

function _retryInit() {
  _initApp()
}

onDestroy(() => {
  cancelUpdateMsgTimeout?.()
  unlistenSidecar?.()
})

async function checkForUpdates() {
  if (!invoke || isCheckingUpdates) return
  isCheckingUpdates = true
  _updateCheckMessage = ''
  try {
    updateInfo = await invoke('check_for_updates')
    if (updateInfo?.updateAvailable) {
      _showUpdateBanner = true
    } else {
      _updateCheckMessage = 'You are up to date!'
      cancelUpdateMsgTimeout?.()
      cancelUpdateMsgTimeout = safeTimeout(() => {
        _updateCheckMessage = ''
      }, 3000)
    }
  } catch (_err) {
    _updateCheckMessage = 'Could not check for updates'
    cancelUpdateMsgTimeout?.()
    cancelUpdateMsgTimeout = safeTimeout(() => {
      _updateCheckMessage = ''
    }, 3000)
  } finally {
    isCheckingUpdates = false
  }
}

async function checkPrerequisites() {
  if (!invoke) return
  try {
    const result = await invoke('check_prerequisites')
    if (!result.allInstalled) {
      _prerequisitesData = result.prerequisites
      _showPrerequisites = true
    }
  } catch (_err) {}
}

async function _openUrl(url) {
  if (!invoke) return
  try {
    await invoke('open_url', { url })
  } catch (_err) {}
}

async function loadProfiles() {
  if (!selectedProject || !invoke) return
  try {
    _profiles = await invoke('list_profiles', { projectKey: selectedProject })
    selectedProfile = ''
  } catch (err) {
    _errorMessage = `Failed to load profiles: ${err}`
  }
}

async function _handleConnect() {
  if (!selectedProject || !selectedProfile || !invoke) return

  _errorMessage = ''
  _connectionStatus = 'connecting'
  _statusMessage = 'Initializing connection...'

  try {
    const result = await invoke('connect', {
      projectKey: selectedProject,
      profile: selectedProfile,
      localPort: null,
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

    _connectionStatus = 'connected'
    _statusMessage = 'Tunnel active'

    // Show save prompt
    lastConnectedConfig = {
      projectKey: selectedProject,
      profile: selectedProfile,
    }
    _showSavePrompt = true
    initSavePrompt()
  } catch (err) {
    _errorMessage = `${err}`
    _connectionStatus = 'disconnected'
    _statusMessage = ''
  }
}

async function _handleSavedConnectionConnect(savedConnection) {
  if (!invoke) return

  _errorMessage = ''
  _connectingId = savedConnection.id
  _connectionStatus = 'connecting'
  _statusMessage = `Connecting to ${savedConnection.name}...`

  try {
    const result = await invoke('connect', {
      projectKey: savedConnection.projectKey,
      profile: savedConnection.profile,
      localPort: null,
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

    _connectionStatus = 'connected'
    _statusMessage = 'Tunnel active'
    _connectingId = null
  } catch (err) {
    _errorMessage = `${err}`
    _connectionStatus =
      activeConnections.length > 0 ? 'connected' : 'disconnected'
    _statusMessage = activeConnections.length > 0 ? 'Tunnel active' : ''
    _connectingId = null
  }
}

async function _handleDisconnect() {
  if (!invoke) return
  try {
    await invoke('disconnect_all')
    activeConnections = []
    _connectionStatus = 'disconnected'
    _statusMessage = ''
    _showSavePrompt = false
  } catch (err) {
    _errorMessage = `Disconnect failed: ${err}`
  }
}

async function _handleDisconnectOne(connectionId) {
  if (!invoke) return
  try {
    await invoke('disconnect', { connectionId })
    activeConnections = activeConnections.filter((c) => c.id !== connectionId)
    if (activeConnections.length === 0) {
      _connectionStatus = 'disconnected'
      _statusMessage = ''
    }
  } catch (err) {
    _errorMessage = `Disconnect failed: ${err}`
  }
}

async function _handleDisconnectAll() {
  if (!invoke) return
  try {
    await invoke('disconnect_all')
    activeConnections = []
    _connectionStatus = 'disconnected'
    _statusMessage = ''
  } catch (err) {
    _errorMessage = `Disconnect all failed: ${err}`
  }
}

function initSavePrompt() {
  if (!lastConnectedConfig) return
  const project = projects.find((p) => p.key === lastConnectedConfig.projectKey)
  saveConnectionName = `${project?.name || lastConnectedConfig.projectKey} - ${lastConnectedConfig.profile}`
}

async function _handleSaveConnection() {
  if (!invoke || !lastConnectedConfig || !saveConnectionName.trim()) return

  try {
    const saved = await invoke('save_connection', {
      name: saveConnectionName.trim(),
      projectKey: lastConnectedConfig.projectKey,
      profile: lastConnectedConfig.profile,
    })
    savedConnections = [
      ...savedConnections.filter((c) => c.id !== saved.id),
      saved,
    ]
    _showSavePrompt = false
    saveConnectionName = ''
  } catch (err) {
    _errorMessage = `Failed to save connection: ${err}`
  }
}

function _handleDeleteSavedConnection(connection) {
  showDeleteConfirm = connection
}

async function _confirmDelete() {
  if (!invoke || !showDeleteConfirm) return
  try {
    await invoke('delete_saved_connection', { id: showDeleteConfirm.id })
    savedConnections = savedConnections.filter(
      (c) => c.id !== showDeleteConfirm.id,
    )
    showDeleteConfirm = null
  } catch (err) {
    _errorMessage = `Failed to delete connection: ${err}`
    showDeleteConfirm = null
  }
}

function _cancelDelete() {
  showDeleteConfirm = null
}

let isUpdating = $state(false)

async function _handleInstallUpdate() {
  if (!invoke || !updateInfo?.updateAvailable || isUpdating) return

  isUpdating = true
  _statusMessage = 'Downloading update...'

  try {
    await invoke('install_update')
    _statusMessage = 'Update installed! Restart to apply.'
    // The app will restart automatically after install
  } catch (err) {
    _errorMessage = `Update failed: ${err}`
    isUpdating = false
    _statusMessage = ''
  }
}

function _handleDismissUpdate() {
  _showUpdateBanner = false
}

function _handleProjectChange(newProject) {
  selectedProject = newProject
  loadProfiles()
}

function _handleProfileChange(newProfile) {
  selectedProfile = newProfile
}

function _dismissError() {
  _errorMessage = ''
}

function _dismissSavePrompt() {
  _showSavePrompt = false
}

// Computed: check if the selected project/profile is already saved
const _isAlreadySaved = $derived(
  savedConnections.some(
    (c) => c.projectKey === selectedProject && c.profile === selectedProfile,
  ),
)
</script>

<main>
  {#if !_ready}
    <div class="loading-screen">
      <svg width="48" height="48" viewBox="0 0 32 32" fill="none">
        <rect width="32" height="32" rx="8" fill="url(#gradient-loading)"/>
        <path d="M10 12h12M10 16h12M10 20h8" stroke="white" stroke-width="2" stroke-linecap="round"/>
        <circle cx="24" cy="20" r="3" fill="white"/>
        <defs>
          <linearGradient id="gradient-loading" x1="0" y1="0" x2="32" y2="32">
            <stop stop-color="#6366f1"/>
            <stop offset="1" stop-color="#8b5cf6"/>
          </linearGradient>
        </defs>
      </svg>
      {#if !_initFailed}
        <div class="loading-spinner"></div>
      {/if}
      <span class="loading-text">{_initStatus}</span>
      {#if _initFailed && _errorMessage}
        <p class="init-error-text">{_errorMessage}</p>
        <button class="btn-retry" onclick={retryInit}>Retry</button>
      {/if}
    </div>
  {:else}
    <div class="app-container">
      {#if showUpdateBanner && updateInfo?.updateAvailable}
        <UpdateBanner
          {updateInfo}
          {isUpdating}
          onInstall={handleInstallUpdate}
          onDismiss={handleDismissUpdate}
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
                <stop stop-color="#6366f1"/>
                <stop offset="1" stop-color="#8b5cf6"/>
              </linearGradient>
            </defs>
          </svg>
        </div>
        <div class="header-text">
          <h1>RDS Connect</h1>
          <p>Secure database tunneling via AWS SSM</p>
        </div>
      </header>

      <div class="main-content">
        {#if savedConnections.length > 0}
          <SavedConnections
            {savedConnections}
            {activeConnections}
            {projects}
            onConnect={handleSavedConnectionConnect}
            onDisconnect={handleDisconnectOne}
            onDelete={handleDeleteSavedConnection}
          />
        {/if}

        {#if activeConnections.length > 0}
          <ActiveConnections
            connections={activeConnections}
            {projects}
            onDisconnect={handleDisconnectOne}
            onDisconnectAll={handleDisconnectAll}
          />
        {/if}

        <ConnectionForm
          {projects}
          {profiles}
          {selectedProject}
          {selectedProfile}
          isConnecting={connectionStatus === 'connecting'}
          isLoadingProjects={loadingProjects}
          onProjectChange={handleProjectChange}
          onProfileChange={handleProfileChange}
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

    {#if showPrerequisites}
      <PrerequisitesCheck
        prerequisites={prerequisitesData}
        onDismiss={() => showPrerequisites = false}
        onOpenUrl={openUrl}
      />
    {/if}

    {#if showSettings}
      <Settings
        onClose={() => showSettings = false}
        {invoke}
      />
    {/if}
  {/if}
</main>

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Inter', sans-serif;
    background: linear-gradient(145deg, #0f0f1a 0%, #1a1a2e 50%, #16162a 100%);
    min-height: 100vh;
    color: #e4e4e7;
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
    border: 2px solid rgba(99, 102, 241, 0.2);
    border-top-color: #6366f1;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    will-change: transform;
  }

  .loading-text {
    font-size: 0.875rem;
    color: #9e9ea7;
  }

  .init-error-text {
    margin: 8px 0 0;
    font-size: 0.8rem;
    color: #fca5a5;
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
    color: white;
    background: linear-gradient(135deg, #6366f1 0%, #8b5cf6 100%);
    border: none;
    border-radius: 10px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s;
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.3);
  }

  .btn-retry:hover {
    transform: translateY(-1px);
    box-shadow: 0 6px 16px rgba(99, 102, 241, 0.4);
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
    background: linear-gradient(135deg, #fff 0%, #a5b4fc 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
  }

  .header-text p {
    margin: 4px 0 0;
    font-size: 0.875rem;
    color: #9e9ea7;
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
    background: rgba(251, 191, 36, 0.1);
    border: 1px solid rgba(251, 191, 36, 0.2);
    border-radius: 12px;
    animation: fadeIn 0.3s ease-out;
  }

  .save-label {
    font-size: 0.875rem;
    color: #fbbf24;
    font-weight: 500;
  }

  .save-input {
    width: 100%;
    padding: 10px 14px;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    color: #e4e4e7;
    font-size: 0.9rem;
    outline: none;
    transition: border-color 0.2s, box-shadow 0.2s;
  }

  .save-input:focus {
    border-color: #fbbf24;
    box-shadow: 0 0 0 2px rgba(251, 191, 36, 0.2);
  }

  .save-input::placeholder {
    color: #9e9ea7;
  }

  .save-prompt-actions {
    display: flex;
    gap: 8px;
  }

  .btn-save {
    padding: 6px 14px;
    font-size: 0.8rem;
    font-weight: 600;
    color: #1a1a2e;
    background: #fbbf24;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-save:hover {
    background: #fcd34d;
  }

  .btn-dismiss-save {
    padding: 6px 14px;
    font-size: 0.8rem;
    font-weight: 500;
    color: #71717a;
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-dismiss-save:hover {
    background: rgba(255, 255, 255, 0.05);
    color: #a1a1aa;
  }

  .error-toast {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 16px;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.2);
    border-radius: 16px;
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
    color: #f87171;
    flex-shrink: 0;
    margin-top: 2px;
  }

  .error-text {
    flex: 1;
    margin: 0;
    font-size: 0.875rem;
    color: #fca5a5;
    line-height: 1.5;
  }

  .dismiss-btn {
    background: transparent;
    border: none;
    color: #71717a;
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
    transition: background-color 0.2s, color 0.2s;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .dismiss-btn:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #a1a1aa;
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
    color: #8b8b95;
  }

  .footer-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .settings-btn {
    padding: 6px;
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 6px;
    color: #71717a;
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s, color 0.2s;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .settings-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    border-color: rgba(255, 255, 255, 0.12);
    color: #a1a1aa;
  }

  .check-updates-btn {
    padding: 6px 12px;
    font-size: 0.7rem;
    font-weight: 500;
    color: #71717a;
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s, color 0.2s;
  }

  .check-updates-btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.05);
    border-color: rgba(255, 255, 255, 0.12);
    color: #a1a1aa;
  }

  .check-updates-btn:disabled {
    opacity: 0.7;
    cursor: wait;
  }

  .check-updates-btn .btn-spinner {
    display: inline-block;
    width: 10px;
    height: 10px;
    border: 1.5px solid rgba(255, 255, 255, 0.3);
    border-top-color: #a1a1aa;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    margin-right: 4px;
  }

  .update-message {
    font-size: 0.75rem;
    color: #10b981;
    animation: fadeIn 0.3s ease-out;
  }
</style>
