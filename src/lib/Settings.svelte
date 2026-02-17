<script>
import { onMount, onDestroy } from 'svelte'
import { trapFocus, safeTimeout } from './utils.js'

const { onClose, invoke, onProjectsChanged } = $props()

let activeTab = $state('projects')
let awsProfiles = $state([])
let rawConfig = $state('')
let projectConfigs = $state({})
let loading = $state(true)
let saving = $state(false)
let error = $state('')
let success = $state('')

// AWS Profile edit modal state
let editingProfile = $state(null)
let editName = $state('')
let editContent = $state('')

// Project edit modal state
let editingProject = $state(null)
let projectKey = $state('')
let projectName = $state('')
let projectRegion = $state('us-east-1')
let projectDatabase = $state('')
let projectSecretPrefix = $state('')
let projectRdsType = $state('cluster')
let projectEngine = $state('postgres')
let projectRdsPattern = $state('')
let projectProfileFilter = $state('')
let projectDefaultPort = $state('5432')
let projectPortMappings = $state([])

// Delete confirmation state
let deleteConfirmProfile = $state(null)
let deleteConfirmProjectKey = $state(null)

// Timeout cleanup
let cancelSuccessTimeout = null

onMount(() => {
  loadData()
})

async function loadData() {
  loading = true
  error = ''
  try {
    const [profiles, config, configs] = await Promise.all([
      invoke('read_aws_config'),
      invoke('get_raw_aws_config'),
      invoke('list_project_configs'),
    ])
    awsProfiles = profiles
    rawConfig = config
    projectConfigs = configs
  } catch (err) {
    error = `Failed to load settings: ${err}`
  } finally {
    loading = false
  }
}

function showSuccess(msg) {
  success = msg
  cancelSuccessTimeout?.()
  cancelSuccessTimeout = safeTimeout(() => { success = '' }, 3000)
}

// ---- AWS Profile functions ----

function openAddProfile() {
  editingProfile = { isNew: true }
  editName = ''
  editContent = 'region = us-east-1\n'
}

function openEditProfile(profile) {
  editingProfile = profile
  editName = profile.name
  editContent = profile.rawContent
}

function closeEditModal() {
  editingProfile = null
  editName = ''
  editContent = ''
}

async function saveProfile() {
  if (!editName.trim()) {
    error = 'Profile name is required'
    return
  }

  saving = true
  error = ''
  try {
    await invoke('save_aws_profile', {
      profile: {
        name: editName.trim(),
        rawContent: editContent,
        region: null,
        sourceProfile: null,
        roleArn: null,
        mfaSerial: null,
        ssoStartUrl: null,
        ssoRegion: null,
        ssoAccountId: null,
        ssoRoleName: null,
      },
    })
    showSuccess('Profile saved successfully')
    closeEditModal()
    await loadData()
  } catch (err) {
    error = `Failed to save profile: ${err}`
  } finally {
    saving = false
  }
}

function requestDeleteProfile(profileName) {
  deleteConfirmProfile = profileName
}

function cancelDeleteProfile() {
  deleteConfirmProfile = null
}

async function confirmDeleteProfile() {
  if (!deleteConfirmProfile) return
  const profileName = deleteConfirmProfile
  deleteConfirmProfile = null

  saving = true
  error = ''
  try {
    await invoke('delete_aws_profile', { profileName })
    showSuccess('Profile deleted')
    await loadData()
  } catch (err) {
    error = `Failed to delete profile: ${err}`
  } finally {
    saving = false
  }
}

async function saveRawConfig() {
  saving = true
  error = ''
  try {
    await invoke('save_raw_aws_config', { content: rawConfig })
    showSuccess('Config saved successfully')
    await loadData()
  } catch (err) {
    error = `Failed to save config: ${err}`
  } finally {
    saving = false
  }
}

// ---- Project config functions ----

function handleEngineChange(e) {
  projectEngine = e.target.value
  projectDefaultPort = projectEngine === 'mysql' ? '3306' : '5432'
}

function openAddProject() {
  editingProject = { isNew: true }
  projectKey = ''
  projectName = ''
  projectRegion = 'us-east-1'
  projectDatabase = ''
  projectSecretPrefix = 'rds!cluster'
  projectRdsType = 'cluster'
  projectEngine = 'postgres'
  projectRdsPattern = ''
  projectProfileFilter = ''
  projectDefaultPort = '5432'
  projectPortMappings = [{ suffix: '', port: '' }]
}

function openEditProject(key, config) {
  editingProject = { isNew: false, key }
  projectKey = key
  projectName = config.name
  projectRegion = config.region
  projectDatabase = config.database
  projectSecretPrefix = config.secretPrefix
  projectRdsType = config.rdsType
  projectEngine = config.engine || 'postgres'
  projectRdsPattern = config.rdsPattern
  projectProfileFilter = config.profileFilter || ''
  projectDefaultPort = config.defaultPort
  const mappings = Object.entries(config.envPortMapping || {}).map(([suffix, port]) => ({ suffix, port }))
  projectPortMappings = mappings.length > 0 ? mappings : [{ suffix: '', port: '' }]
}

function closeProjectModal() {
  editingProject = null
}

function addPortMapping() {
  projectPortMappings = [...projectPortMappings, { suffix: '', port: '' }]
}

function removePortMapping(index) {
  projectPortMappings = projectPortMappings.filter((_, i) => i !== index)
}

async function saveProject() {
  if (!projectKey.trim()) {
    error = 'Project key is required'
    return
  }
  if (!projectName.trim()) {
    error = 'Project name is required'
    return
  }

  const envPortMapping = {}
  for (const m of projectPortMappings) {
    if (m.suffix.trim() && m.port.trim()) {
      envPortMapping[m.suffix.trim()] = m.port.trim()
    }
  }

  const config = {
    name: projectName.trim(),
    region: projectRegion.trim(),
    database: projectDatabase.trim(),
    secretPrefix: projectSecretPrefix.trim(),
    rdsType: projectRdsType,
    engine: projectEngine,
    rdsPattern: projectRdsPattern.trim(),
    profileFilter: projectProfileFilter.trim() || null,
    envPortMapping,
    defaultPort: projectDefaultPort.trim(),
  }

  saving = true
  error = ''
  try {
    await invoke('save_project_config', { key: projectKey.trim(), config })
    showSuccess('Project saved')
    closeProjectModal()
    await loadData()
    onProjectsChanged?.()
  } catch (err) {
    error = `Failed to save project: ${err}`
  } finally {
    saving = false
  }
}

function requestDeleteProject(key) {
  deleteConfirmProjectKey = key
}

function cancelDeleteProject() {
  deleteConfirmProjectKey = null
}

async function confirmDeleteProject() {
  if (!deleteConfirmProjectKey) return
  const key = deleteConfirmProjectKey
  deleteConfirmProjectKey = null

  saving = true
  error = ''
  try {
    await invoke('delete_project_config', { key })
    showSuccess('Project deleted')
    await loadData()
    onProjectsChanged?.()
  } catch (err) {
    error = `Failed to delete project: ${err}`
  } finally {
    saving = false
  }
}

// ---- Keyboard handlers ----

function handleOverlayKeydown(e) {
  if (e.key === 'Escape') {
    if (editingProfile || editingProject) {
      closeEditModal()
      closeProjectModal()
    } else {
      onClose()
    }
  }
}

function handleEditOverlayKeydown(e) {
  if (e.key === 'Escape') {
    closeEditModal()
    closeProjectModal()
  }
}

onDestroy(() => {
  cancelSuccessTimeout?.()
})
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="settings-modal" role="dialog" aria-label="Settings" tabindex="-1" onkeydown={handleOverlayKeydown}>
  <div class="modal-content" use:trapFocus>
    <div class="modal-header">
      <h2>Settings</h2>
      <button class="close-btn" onclick={onClose} aria-label="Close settings">
        <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
          <path d="M5 5l10 10M15 5l-10 10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
        </svg>
      </button>
    </div>

    <div class="tabs">
      <button
        class="tab"
        class:active={activeTab === 'projects'}
        onclick={() => activeTab = 'projects'}
      >
        Projects
      </button>
      <button
        class="tab"
        class:active={activeTab === 'profiles'}
        onclick={() => activeTab = 'profiles'}
      >
        AWS Profiles
      </button>
      <button
        class="tab"
        class:active={activeTab === 'raw'}
        onclick={() => activeTab = 'raw'}
      >
        Raw Config
      </button>
    </div>

    {#if error}
      <div class="message error">{error}</div>
    {/if}
    {#if success}
      <div class="message success">{success}</div>
    {/if}

    <div class="tab-content">
      {#if loading}
        <div class="loading">Loading...</div>
      {:else if activeTab === 'projects'}
        <div class="profiles-tab">
          <div class="profiles-header">
            <span class="profiles-path">~/.rds-ssm-connect/projects.json</span>
            <button class="btn-add" onclick={openAddProject}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
              </svg>
              Add Project
            </button>
          </div>

          {#if Object.keys(projectConfigs).length === 0}
            <div class="empty-state">
              <p>No projects configured</p>
              <p class="hint">Click "Add Project" to get started</p>
            </div>
          {:else}
            <div class="profiles-list">
              {#each Object.entries(projectConfigs) as [key, config]}
                <div class="profile-card">
                  <div class="profile-header">
                    <span class="profile-name">{config.name}</span>
                    <div class="profile-actions">
                      <button class="btn-icon" onclick={() => openEditProject(key, config)} aria-label="Edit {config.name}">
                        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                          <path d="M10.5 1.5l2 2-8 8H2.5v-2l8-8z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                      </button>
                      <button class="btn-icon delete" onclick={() => requestDeleteProject(key)} aria-label="Delete {config.name}">
                        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                          <path d="M2 4h10M5 4V2.5a.5.5 0 01.5-.5h3a.5.5 0 01.5.5V4M11 4v8a1 1 0 01-1 1H4a1 1 0 01-1-1V4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                      </button>
                    </div>
                  </div>
                  {#if deleteConfirmProjectKey === key}
                    <div class="inline-confirm">
                      <span>Delete "{config.name}"?</span>
                      <div class="inline-confirm-actions">
                        <button class="btn-inline-confirm" onclick={confirmDeleteProject}>Delete</button>
                        <button class="btn-inline-cancel" onclick={cancelDeleteProject}>Cancel</button>
                      </div>
                    </div>
                  {:else}
                    <div class="profile-details">
                      <span class="detail">{config.region}</span>
                      <span class="detail">{config.rdsType}</span>
                      <span class="detail">{config.engine || 'postgres'}</span>
                      <span class="detail">{config.database}</span>
                      <span class="detail">{Object.keys(config.envPortMapping || {}).length} port mappings</span>
                    </div>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {:else if activeTab === 'profiles'}
        <div class="profiles-tab">
          <div class="profiles-header">
            <span class="profiles-path">~/.aws/config</span>
            <button class="btn-add" onclick={openAddProfile}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
              </svg>
              Add Profile
            </button>
          </div>

          {#if awsProfiles.length === 0}
            <div class="empty-state">
              <p>No AWS profiles found</p>
              <p class="hint">Click "Add Profile" to create one</p>
            </div>
          {:else}
            <div class="profiles-list">
              {#each awsProfiles as profile}
                <div class="profile-card">
                  <div class="profile-header">
                    <span class="profile-name">{profile.name}</span>
                    <div class="profile-actions">
                      <button class="btn-icon" onclick={() => openEditProfile(profile)} aria-label="Edit {profile.name}">
                        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                          <path d="M10.5 1.5l2 2-8 8H2.5v-2l8-8z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                      </button>
                      <button class="btn-icon delete" onclick={() => requestDeleteProfile(profile.name)} aria-label="Delete {profile.name}">
                        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                          <path d="M2 4h10M5 4V2.5a.5.5 0 01.5-.5h3a.5.5 0 01.5.5V4M11 4v8a1 1 0 01-1 1H4a1 1 0 01-1-1V4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                      </button>
                    </div>
                  </div>
                  {#if deleteConfirmProfile === profile.name}
                    <div class="inline-confirm">
                      <span>Delete "{profile.name}"?</span>
                      <div class="inline-confirm-actions">
                        <button class="btn-inline-confirm" onclick={confirmDeleteProfile}>Delete</button>
                        <button class="btn-inline-cancel" onclick={cancelDeleteProfile}>Cancel</button>
                      </div>
                    </div>
                  {:else}
                    <div class="profile-details">
                      {#if profile.region}
                        <span class="detail">Region: {profile.region}</span>
                      {/if}
                      {#if profile.sourceProfile}
                        <span class="detail">Source: {profile.sourceProfile}</span>
                      {/if}
                      {#if profile.roleArn}
                        <span class="detail">Role: {profile.roleArn.split('/').pop()}</span>
                      {/if}
                      {#if profile.ssoStartUrl}
                        <span class="detail">SSO</span>
                      {/if}
                    </div>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {:else}
        <div class="raw-tab">
          <textarea
            class="raw-editor"
            bind:value={rawConfig}
            placeholder="# AWS Config file contents..."
            spellcheck="false"
          ></textarea>
          <div class="raw-actions">
            <button class="btn-save" onclick={saveRawConfig} disabled={saving}>
              {saving ? 'Saving...' : 'Save Config'}
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>

  <!-- Edit AWS Profile Modal -->
  {#if editingProfile}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="edit-modal-overlay" onclick={closeEditModal} onkeydown={handleEditOverlayKeydown}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="edit-modal" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()} use:trapFocus role="dialog" tabindex="-1" aria-label={editingProfile.isNew ? 'Add profile' : 'Edit profile'}>
        <h3>{editingProfile.isNew ? 'Add Profile' : 'Edit Profile'}</h3>

        <div class="form-group">
          <label for="profile-name">Profile Name</label>
          <input
            id="profile-name"
            type="text"
            bind:value={editName}
            placeholder="my-profile"
            disabled={!editingProfile.isNew}
          />
        </div>

        <div class="form-group">
          <label for="profile-content">Configuration</label>
          <textarea
            id="profile-content"
            bind:value={editContent}
            placeholder="region = us-east-1&#10;source_profile = default&#10;role_arn = arn:aws:iam::..."
            spellcheck="false"
          ></textarea>
        </div>

        <div class="edit-actions">
          <button class="btn-cancel" onclick={closeEditModal}>Cancel</button>
          <button class="btn-save" onclick={saveProfile} disabled={saving}>
            {saving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Edit Project Modal -->
  {#if editingProject}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="edit-modal-overlay" onclick={closeProjectModal} onkeydown={handleEditOverlayKeydown}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="edit-modal project-modal" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()} use:trapFocus role="dialog" tabindex="-1" aria-label={editingProject.isNew ? 'Add project' : 'Edit project'}>
        <h3>{editingProject.isNew ? 'Add Project' : 'Edit Project'}</h3>

        <div class="project-form-scroll">
          <div class="form-group">
            <label for="project-key">Project Key</label>
            <input
              id="project-key"
              type="text"
              bind:value={projectKey}
              placeholder="my-project"
              disabled={!editingProject.isNew}
            />
            {#if editingProject.isNew}
              <span class="field-hint">Lowercase letters, digits, and hyphens</span>
            {/if}
          </div>

          <div class="form-row">
            <div class="form-group">
              <label for="project-name">Name</label>
              <input id="project-name" type="text" bind:value={projectName} placeholder="My Project" />
            </div>
            <div class="form-group">
              <label for="project-region">Region</label>
              <input id="project-region" type="text" bind:value={projectRegion} placeholder="us-east-1" />
            </div>
          </div>

          <div class="form-row">
            <div class="form-group">
              <label for="project-database">Database</label>
              <input id="project-database" type="text" bind:value={projectDatabase} placeholder="mydb" />
            </div>
            <div class="form-group">
              <label for="project-secret-prefix">Secret Prefix</label>
              <input id="project-secret-prefix" type="text" bind:value={projectSecretPrefix} placeholder="rds!cluster" />
            </div>
          </div>

          <div class="form-row">
            <div class="form-group">
              <label for="project-rds-type">RDS Type</label>
              <select id="project-rds-type" bind:value={projectRdsType}>
                <option value="cluster">Cluster (Aurora)</option>
                <option value="instance">Instance</option>
              </select>
            </div>
            <div class="form-group">
              <label for="project-engine">Engine</label>
              <select id="project-engine" value={projectEngine} onchange={handleEngineChange}>
                <option value="postgres">PostgreSQL</option>
                <option value="mysql">MySQL</option>
              </select>
            </div>
          </div>

          <div class="form-group">
            <label for="project-rds-pattern">RDS Pattern</label>
            <input id="project-rds-pattern" type="text" bind:value={projectRdsPattern} placeholder="-rds-aurora" />
          </div>

          <div class="form-row">
            <div class="form-group">
              <label for="project-profile-filter">Profile Filter</label>
              <input id="project-profile-filter" type="text" bind:value={projectProfileFilter} placeholder="(optional)" />
            </div>
            <div class="form-group">
              <label for="project-default-port">Default Port</label>
              <input id="project-default-port" type="text" bind:value={projectDefaultPort} placeholder="5432" />
            </div>
          </div>

          <div class="port-mappings">
            <div class="port-mappings-header">
              <span class="port-mappings-label">Port Mappings</span>
              <button class="btn-add-small" onclick={addPortMapping} type="button">+ Add</button>
            </div>
            {#each projectPortMappings as mapping, i}
              <div class="port-mapping-row">
                <input type="text" bind:value={mapping.suffix} placeholder="env suffix" />
                <input type="text" bind:value={mapping.port} placeholder="port" />
                <button class="btn-remove" onclick={() => removePortMapping(i)} type="button" aria-label="Remove mapping">
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                    <path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                  </svg>
                </button>
              </div>
            {/each}
          </div>
        </div>

        <div class="edit-actions">
          <button class="btn-cancel" onclick={closeProjectModal}>Cancel</button>
          <button class="btn-save" onclick={saveProject} disabled={saving}>
            {saving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .settings-modal {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.8);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 24px;
  }

  .modal-content {
    background: rgba(26, 26, 46, 0.85);
    -webkit-backdrop-filter: var(--glass-blur-heavy);
    backdrop-filter: var(--glass-blur-heavy);
    border: 1px solid var(--glass-border);
    border-radius: 20px;
    padding: 24px;
    max-width: 560px;
    width: 100%;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    box-shadow: var(--glass-inner-glow), var(--glass-shadow);
    animation: slideUp 0.3s ease-out;
  }

  @keyframes slideUp {
    from { opacity: 0; transform: translateY(20px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 20px;
  }

  .modal-header h2 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 600;
    color: #e4e4e7;
  }

  .close-btn {
    background: none;
    border: none;
    color: #71717a;
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
    transition: background-color 0.2s, color 0.2s;
  }

  .close-btn:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #e4e4e7;
  }

  .tabs {
    display: flex;
    gap: 4px;
    background: rgba(255, 255, 255, 0.03);
    padding: 4px;
    border-radius: 10px;
    margin-bottom: 16px;
  }

  .tab {
    flex: 1;
    padding: 10px 16px;
    font-size: 0.875rem;
    font-weight: 500;
    color: #71717a;
    background: none;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .tab:hover {
    color: #a1a1aa;
    background: var(--glass-bg-hover);
  }

  .tab.active {
    background: rgba(99, 102, 241, 0.15);
    color: #a5b4fc;
  }

  .message {
    padding: 10px 14px;
    border-radius: 8px;
    font-size: 0.875rem;
    margin-bottom: 12px;
  }

  .message.error {
    background: rgba(239, 68, 68, 0.1);
    color: #f87171;
    border: 1px solid rgba(239, 68, 68, 0.2);
  }

  .message.success {
    background: rgba(34, 197, 94, 0.1);
    color: #4ade80;
    border: 1px solid rgba(34, 197, 94, 0.2);
  }

  .tab-content {
    flex: 1;
    overflow-y: auto;
    min-height: 300px;
  }

  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 200px;
    color: #9e9ea7;
  }

  .profiles-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .profiles-path {
    font-size: 0.75rem;
    color: #8b8b95;
    font-family: ui-monospace, monospace;
  }

  .btn-add {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    font-size: 0.8rem;
    font-weight: 500;
    color: #a5b4fc;
    background: rgba(99, 102, 241, 0.1);
    border: 1px solid rgba(99, 102, 241, 0.2);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-add:hover {
    background: rgba(99, 102, 241, 0.15);
  }

  .btn-add:active {
    transform: var(--press-scale);
  }

  .empty-state {
    text-align: center;
    padding: 40px 20px;
    color: #9e9ea7;
  }

  .empty-state p {
    margin: 0 0 8px;
  }

  .empty-state .hint {
    font-size: 0.875rem;
    color: #8b8b95;
  }

  .profiles-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .profile-card {
    background: var(--glass-bg);
    border: 1px solid var(--glass-border);
    border-radius: 12px;
    padding: 14px;
  }

  .profile-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .profile-name {
    font-weight: 500;
    color: #e4e4e7;
  }

  .profile-actions {
    display: flex;
    gap: 4px;
  }

  .btn-icon {
    padding: 6px;
    background: none;
    border: none;
    color: #71717a;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-icon:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #a1a1aa;
  }

  .btn-icon.delete:hover {
    background: rgba(239, 68, 68, 0.1);
    color: #f87171;
  }

  .inline-confirm {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 0 0;
  }

  .inline-confirm span {
    font-size: 0.8rem;
    color: #f87171;
  }

  .inline-confirm-actions {
    display: flex;
    gap: 6px;
  }

  .btn-inline-confirm {
    padding: 4px 10px;
    font-size: 0.75rem;
    font-weight: 600;
    color: white;
    background: #ef4444;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-inline-confirm:hover {
    background: #dc2626;
  }

  .btn-inline-cancel {
    padding: 4px 10px;
    font-size: 0.75rem;
    font-weight: 500;
    color: #9e9ea7;
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-inline-cancel:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .profile-details {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .detail {
    font-size: 0.75rem;
    color: #9e9ea7;
    background: rgba(255, 255, 255, 0.05);
    padding: 4px 8px;
    border-radius: 4px;
  }

  .raw-tab {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .raw-editor {
    flex: 1;
    min-height: 280px;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 10px;
    padding: 14px;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    color: #e4e4e7;
    resize: none;
    outline: none;
  }

  .raw-editor:focus {
    border-color: #6366f1;
  }

  .raw-actions {
    margin-top: 12px;
    display: flex;
    justify-content: flex-end;
  }

  .btn-save {
    padding: 10px 20px;
    font-size: 0.875rem;
    font-weight: 500;
    color: white;
    background: linear-gradient(135deg, #6366f1 0%, #8b5cf6 100%);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s;
  }

  .btn-save:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.3);
  }

  .btn-save:active:not(:disabled) {
    transform: var(--press-scale);
  }

  .btn-save:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Edit Modal */
  .edit-modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 110;
  }

  .edit-modal {
    background: rgba(30, 30, 50, 0.9);
    -webkit-backdrop-filter: var(--glass-blur-heavy);
    backdrop-filter: var(--glass-blur-heavy);
    border: 1px solid var(--glass-border);
    border-radius: 16px;
    padding: 24px;
    width: 400px;
    max-width: 90%;
    box-shadow: var(--glass-inner-glow), var(--glass-shadow);
  }

  .edit-modal.project-modal {
    width: 480px;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
  }

  .project-form-scroll {
    flex: 1;
    overflow-y: auto;
    margin-bottom: 8px;
  }

  .edit-modal h3 {
    margin: 0 0 20px;
    font-size: 1.1rem;
    color: #e4e4e7;
  }

  .form-group {
    margin-bottom: 16px;
  }

  .form-group label {
    display: block;
    font-size: 0.8rem;
    font-weight: 500;
    color: #a1a1aa;
    margin-bottom: 8px;
  }

  .form-group input,
  .form-group textarea,
  .form-group select {
    width: 100%;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 0.875rem;
    color: #e4e4e7;
    outline: none;
  }

  .form-group select {
    cursor: pointer;
    appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' fill='%2371717a' viewBox='0 0 16 16'%3E%3Cpath d='M4 6l4 4 4-4'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 12px center;
    padding-right: 32px;
  }

  .form-group input:focus,
  .form-group textarea:focus,
  .form-group select:focus {
    border-color: #6366f1;
  }

  .form-group input:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .form-group textarea {
    min-height: 150px;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    resize: vertical;
  }

  .field-hint {
    display: block;
    font-size: 0.7rem;
    color: #8b8b95;
    margin-top: 4px;
  }

  .form-row {
    display: flex;
    gap: 12px;
  }

  .form-row .form-group {
    flex: 1;
  }

  .port-mappings {
    margin-bottom: 16px;
  }

  .port-mappings-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .port-mappings-label {
    font-size: 0.8rem;
    font-weight: 500;
    color: #a1a1aa;
  }

  .btn-add-small {
    padding: 4px 10px;
    font-size: 0.75rem;
    font-weight: 500;
    color: #a5b4fc;
    background: rgba(99, 102, 241, 0.1);
    border: 1px solid rgba(99, 102, 241, 0.2);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-add-small:hover {
    background: rgba(99, 102, 241, 0.15);
  }

  .port-mapping-row {
    display: flex;
    gap: 8px;
    margin-bottom: 6px;
    align-items: center;
  }

  .port-mapping-row input {
    flex: 1;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    padding: 8px 10px;
    font-size: 0.8rem;
    color: #e4e4e7;
    outline: none;
  }

  .port-mapping-row input:focus {
    border-color: #6366f1;
  }

  .btn-remove {
    padding: 6px;
    background: none;
    border: none;
    color: #71717a;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
    flex-shrink: 0;
  }

  .btn-remove:hover {
    background: rgba(239, 68, 68, 0.1);
    color: #f87171;
  }

  .edit-actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
    margin-top: 20px;
  }

  .btn-cancel {
    padding: 10px 20px;
    font-size: 0.875rem;
    font-weight: 500;
    color: #71717a;
    background: none;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-cancel:hover {
    background: rgba(255, 255, 255, 0.05);
    color: #a1a1aa;
  }

  .btn-cancel:active {
    transform: var(--press-scale);
  }
</style>
