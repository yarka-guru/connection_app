<script>
const {
  projects = [],
  profiles = [],
  selectedProject = '',
  selectedProfile = '',
  selectedDatabase = '',
  isConnecting = false,
  isLoadingProjects = false,
  onProjectChange,
  onProfileChange,
  onDatabaseChange,
  onConnect,
} = $props()

// Get databases for the selected project
const currentProjectDatabases = $derived(() => {
  if (!selectedProject) return []
  const project = projects.find((p) => p.key === selectedProject)
  return project?.databases || []
})

const hasDatabases = $derived(currentProjectDatabases().length > 1)

const canConnect = $derived(
  selectedProject && selectedProfile && !isConnecting && (!hasDatabases || selectedDatabase),
)

function handleProjectSelect(e) {
  onProjectChange?.(e.target.value)
}

function handleProfileSelect(e) {
  onProfileChange?.(e.target.value)
}

function handleDatabaseSelect(e) {
  onDatabaseChange?.(e.target.value)
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
          {#each projects as project}
            <option value={project.key}>{project.name}</option>
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
      <label for="profile">
        <span class="label-text">AWS Profile</span>
      </label>
      <div class="select-wrapper">
        <select
          id="profile"
          value={selectedProfile}
          onchange={handleProfileSelect}
          disabled={!selectedProject || isConnecting}
        >
          <option value="">Choose an AWS profile</option>
          {#each profiles as profile}
            <option value={profile}>{profile}</option>
          {/each}
        </select>
        <div class="select-icon">
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </div>
      </div>
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
            {#each currentProjectDatabases() as db}
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
