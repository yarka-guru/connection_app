<script>
let {
  projects = [],
  selectedProject = $bindable(''),
  selectedEnvironment = $bindable(''),
  profileOverride = $bindable(''),
  selectedDatabase = $bindable(''),
  isConnecting = false,
  isLoadingProjects = false,
  onCreateProfile,
  onSelectProfile,
  onConnect,
} = $props()

// Only reachable projects are listed; broken ones stay visible but disabled.
const listedProjects = $derived(projects.filter((p) => p.reachable))

const currentProject = $derived(projects.find((p) => p.key === selectedProject))
const currentEnvironments = $derived(currentProject?.environments ?? [])
const currentResolved = $derived(
  currentEnvironments.find((e) => e.environment.name === selectedEnvironment),
)
const currentDatabases = $derived(currentProject?.databases || [])
const hasDatabases = $derived(currentDatabases.length > 1)

// The profile that will be used: the user's choice, else the resolved one.
const effectiveProfile = $derived(profileOverride || currentResolved?.profile || '')
const showProfilePicker = $derived((currentResolved?.candidates?.length ?? 0) > 1)
const needsProfile = $derived(!!currentResolved && !currentResolved.profile)

const canConnect = $derived(
  !!currentProject &&
    !currentProject.broken &&
    !!currentResolved &&
    !!effectiveProfile &&
    !isConnecting &&
    (!hasDatabases || selectedDatabase),
)

function shortAccount(accountId) {
  if (!accountId || accountId.length < 9) return accountId || 'unknown account'
  return `${accountId.slice(0, 4)}…${accountId.slice(-4)}`
}

function projectLabel(project) {
  const reason = project.broken || project.problems?.[0]?.message
  return reason ? `${project.name} — ${reason}` : project.name
}

function environmentLabel(resolved) {
  if (resolved.profile) return resolved.environment.name
  return `${resolved.environment.name} — no AWS profile for ${shortAccount(resolved.environment.accountId)}`
}

function handleProjectSelect(e) {
  selectedProject = e.target.value
  selectedEnvironment = ''
  profileOverride = ''
  selectedDatabase = ''
}

function handleEnvironmentSelect(e) {
  selectedEnvironment = e.target.value
  profileOverride = ''
}

function handleViaProfileSelect(e) {
  const profile = e.target.value
  if (!profile || !currentResolved) return
  onSelectProfile?.(currentResolved.environment.accountId, profile)
}

function handleDatabaseSelect(e) {
  selectedDatabase = e.target.value
}

function handleConnectClick() {
  onConnect?.()
}
</script>

<div class="connection-card">
  <div class="card-header">
    <div class="card-icon">
      <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
        <path d="M3 5a2 2 0 012-2h10a2 2 0 012 2v10a2 2 0 01-2 2H5a2 2 0 01-2-2V5z" stroke="currentColor" stroke-width="1.5"/>
        <circle cx="7" cy="7" r="1.5" fill="currentColor"/>
        <circle cx="13" cy="7" r="1.5" fill="currentColor"/>
        <path d="M7 13h6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
      </svg>
    </div>
    <span class="card-title">Connection</span>
  </div>

  <div class="form-fields">
    <div class="field-group">
      <label for="project">
        <span class="label-text">Project</span>
      </label>
      <div class="select-wrapper">
        <select
          id="project"
          value={selectedProject}
          onchange={handleProjectSelect}
          disabled={isConnecting || isLoadingProjects}
        >
          <option value="">{isLoadingProjects ? 'Loading projects...' : 'Choose a project'}</option>
          {#each listedProjects as project (project.key)}
            <option value={project.key} disabled={!!project.broken}>{projectLabel(project)}</option>
          {/each}
        </select>
        <div class="select-icon">
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </div>
      </div>
    </div>

    <div class="field-group">
      <label for="environment">
        <span class="label-text">Environment</span>
      </label>
      <div class="env-row">
        <div class="select-wrapper">
          <select
            id="environment"
            value={selectedEnvironment}
            onchange={handleEnvironmentSelect}
            disabled={!selectedProject || isConnecting}
          >
            <option value="">Choose an environment</option>
            {#each currentEnvironments as resolved (resolved.environment.name)}
              <option value={resolved.environment.name} class:no-profile={!resolved.profile}>{environmentLabel(resolved)}</option>
            {/each}
          </select>
          <div class="select-icon">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </div>
        </div>
        {#if needsProfile}
          <button
            class="btn-create-profile"
            type="button"
            disabled={isConnecting}
            onclick={() => onCreateProfile?.(currentResolved.environment.accountId)}
          >
            Create profile
          </button>
        {/if}
      </div>
      {#if showProfilePicker}
        <div class="via-profile">
          <label for="via-profile" class="via-label">via profile</label>
          <div class="select-wrapper">
            <select
              id="via-profile"
              class="select-compact"
              value={effectiveProfile}
              onchange={handleViaProfileSelect}
              disabled={isConnecting}
            >
              {#each currentResolved.candidates as candidate (candidate)}
                <option value={candidate}>{candidate}</option>
              {/each}
            </select>
            <div class="select-icon">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
            </div>
          </div>
        </div>
      {/if}
    </div>

    {#if hasDatabases}
      <div class="field-group">
        <label for="database">
          <span class="label-text">Database</span>
        </label>
        <div class="select-wrapper">
          <select
            id="database"
            value={selectedDatabase}
            onchange={handleDatabaseSelect}
            disabled={!selectedProject || isConnecting}
          >
            <option value="">Choose a database</option>
            {#each currentDatabases as db}
              <option value={db}>{db}</option>
            {/each}
          </select>
          <div class="select-icon">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </div>
        </div>
      </div>
    {/if}
  </div>

  <div class="action-area">
    <button
      class="btn btn-connect"
      onclick={handleConnectClick}
      disabled={!canConnect}
    >
      {#if isConnecting}
        <div class="spinner"></div>
        <span>Connecting...</span>
      {:else}
        <svg width="18" height="18" viewBox="0 0 18 18" fill="none">
          <path d="M9 3v12M3 9h12" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
        </svg>
        <span>Connect</span>
      {/if}
    </button>
  </div>
</div>

<style>
  .connection-card {
    background: var(--glass-bg);
    -webkit-backdrop-filter: var(--glass-blur);
    backdrop-filter: var(--glass-blur);
    border: 1px solid var(--glass-border);
    border-radius: 20px;
    padding: 24px;
    box-shadow: var(--glass-inner-glow);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 20px;
  }

  .card-icon {
    width: 36px;
    height: 36px;
    background: linear-gradient(135deg, rgba(var(--accent-primary-rgb), 0.2) 0%, rgba(var(--accent-secondary-rgb), 0.2) 100%);
    border-radius: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--accent-primary-light);
  }

  .card-title {
    font-size: 1rem;
    font-weight: 600;
    color: var(--text-primary);
  }

  .form-fields {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .field-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  label {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .label-text {
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--text-hover);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .select-wrapper {
    position: relative;
  }

  select {
    width: 100%;
    appearance: none;
    background: rgba(var(--glass-rgb), 0.05);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 12px;
    padding: 14px 44px 14px 16px;
    font-size: 0.95rem;
    color: var(--text-primary);
    cursor: pointer;
    transition: background-color 0.2s ease, border-color 0.2s ease, box-shadow 0.2s ease;
  }

  select:hover:not(:disabled) {
    background: rgba(var(--glass-rgb), 0.08);
    border-color: rgba(var(--glass-rgb), 0.15);
  }

  select:focus {
    outline: none;
    border-color: var(--accent-primary);
    box-shadow: 0 0 0 3px rgba(var(--accent-primary-rgb), 0.15);
  }

  select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  select option {
    background: var(--bg-secondary);
    color: var(--text-primary);
    padding: 12px;
  }

  .env-row {
    display: flex;
    gap: 8px;
    align-items: stretch;
  }

  .env-row .select-wrapper {
    flex: 1;
    min-width: 0;
  }

  .btn-create-profile {
    flex-shrink: 0;
    padding: 0 14px;
    background: rgba(var(--accent-primary-rgb), 0.15);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.4);
    border-radius: 12px;
    color: var(--accent-primary-light);
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
    transition: background-color 0.2s ease, border-color 0.2s ease;
  }

  .btn-create-profile:hover:not(:disabled) {
    background: rgba(var(--accent-primary-rgb), 0.25);
  }

  .btn-create-profile:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .via-profile {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .via-label {
    font-size: 0.8rem;
    color: #9e9ea7;
    white-space: nowrap;
  }

  .via-profile .select-wrapper {
    flex: 1;
    min-width: 0;
  }

  .select-compact {
    padding: 8px 36px 8px 12px;
    font-size: 0.85rem;
    border-radius: 10px;
  }

  select option.no-profile {
    color: #9e9ea7;
  }

  select option:disabled {
    color: #9e9ea7;
  }

  .select-icon {
    position: absolute;
    right: 14px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--text-muted);
    pointer-events: none;
  }

  .action-area {
    margin-top: 24px;
  }

  .btn {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 16px 24px;
    border: none;
    border-radius: 14px;
    font-size: 1rem;
    font-weight: 600;
    cursor: pointer;
    transition: transform 0.2s ease, box-shadow 0.2s ease, opacity 0.2s ease;
  }

  .btn-connect {
    background: var(--bg-button-gradient);
    color: white;
    box-shadow: 0 4px 15px var(--bg-button-gradient-shadow);
  }

  .btn-connect:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 6px 20px rgba(var(--accent-primary-rgb), 0.4);
  }

  .btn-connect:active:not(:disabled) {
    transform: var(--press-scale);
  }

  .btn-connect:disabled {
    opacity: 0.5;
    cursor: not-allowed;
    transform: none;
    box-shadow: none;
  }

  .spinner {
    width: 18px;
    height: 18px;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: white;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
