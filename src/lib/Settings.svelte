<script>
import { onMount, onDestroy } from 'svelte'
import { trapFocus, safeTimeout } from './utils.js'
import { themes, darkThemeNames, lightThemeNames } from './themes.js'
import ThemeSwitcher from './ThemeSwitcher.svelte'
import EnvironmentsEditor from './EnvironmentsEditor.svelte'
import ProjectFileRepair from './ProjectFileRepair.svelte'

const {
  onClose,
  invoke,
  onProjectsChanged,
  onOpenMigration,
  onImport,
  needsMigration = false,
  settingsVersion = 0,
  currentTheme = 'forest',
  onThemeChange,
  currentLightTheme = 'light',
  onLightThemeChange,
  scheme = 'dark',
  onSchemeChange,
  appVersion = '',
  initialProfileTemplate = null,
} = $props()

// profileFilter written for new projects so 3.7.x lists no profiles for them
const HIDE_FROM_LEGACY_FILTER = '__connection-app-3.8__'
const NEW_KEY_PATTERN = /^[a-z0-9][a-z0-9-]*$/
const VALIDATE_DELAY_MS = 300

const AUTH_HINTS = {
  auto: 'Uses an IAM token or the password, whichever the database asks for',
  iam: 'Signs in with an IAM token as the IAM DB user; never reads a secret',
  password: 'Signs in with the password from the secret',
}

// Collapsible section state for project form
let sectionGeneral = $state(true)
let sectionDatabase = $state(true)
let sectionService = $state(true)
let sectionEnvironments = $state(true)
let sectionAdvanced = $state(false)

let activeTab = $state('projects')
let awsProfiles = $state([])
let rawConfig = $state('')
let overview = $state({ projects: [], fileError: null, needsMigration: false, backups: [] })
let entries = $state([])
let accounts = $state([])
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
let projectDatabases = $state([])
let projectSecretPrefix = $state('')
let projectAuthMode = $state('auto')
let projectIamUsername = $state('')
let projectBaseConfig = $state.raw({})
let projectRdsType = $state('cluster')
let projectEngine = $state('postgres')
let projectRdsPattern = $state('')
let projectBastionPattern = $state('')
let projectDefaultPort = $state('5432')
let projectEnvironments = $state([])
let projectSecretPath = $state('')
let projectSecretUsernameField = $state('')
let projectSecretPasswordField = $state('')
let projectMultiplexed = $state(true)

// Service-specific state
let projectConnectionType = $state('rds')
let projectServiceType = $state('vnc')
let projectRemotePort = $state('')
let projectTargetType = $state('ec2-direct')
let projectTargetPattern = $state('')
let projectEcsCluster = $state('')
let projectEcsService = $state('')
let projectSshUsername = $state('')
let projectSshKeyPath = $state('')

// Backend validation of the project form
let validationErrors = $state([])
let validationSeq = 0

// Single-project JSON editor (for broken projects)
let jsonEditKey = $state(null)
let jsonEditText = $state('')
let jsonEditError = $state('')

// Delete confirmation state
let deleteConfirmProfile = $state(null)
let deleteConfirmProjectKey = $state(null)

// Timeout cleanup
let cancelSuccessTimeout = null

const summaryByKey = $derived(new Map(overview.projects.map((p) => [p.key, p])))

// accountId -> the profile the app resolves it to, for accounts with several
const chosenProfiles = $derived.by(() => {
  const chosen = {}
  for (const p of overview.projects) {
    for (const env of p.environments) {
      if (env.profile && env.candidates.length > 1) chosen[env.environment.accountId] = env.profile
    }
  }
  return chosen
})

// Other projects whose environments can be copied into the form
const copySources = $derived(
  entries
    .filter((e) => e.config?.environments?.length > 0 && e.key !== editingProject?.key)
    .map((e) => ({ key: e.key, name: e.config.name || e.key, environments: e.config.environments })),
)

// The project being edited was stored without environments (profile-name based).
// A broken project whose environments could not be read is not legacy.
const projectIsLegacy = $derived(
  !!editingProject
    && !editingProject.isNew
    && !(projectBaseConfig.environments?.length > 0)
    && !editingProject.broken?.hadEnvironments,
)

// Set when the form edits a broken project and the user accepted losing fields
let brokenSaveConfirmed = $state(false)

onMount(() => {
  loadData()
  if (initialProfileTemplate) {
    activeTab = 'profiles'
    openProfileFromTemplate(initialProfileTemplate)
  }
})

// Reload when App changed projects.json behind this screen (migration, import)
let seenVersion = settingsVersion
$effect(() => {
  if (settingsVersion !== seenVersion) {
    seenVersion = settingsVersion
    loadData()
  }
})

async function startImport() {
  error = ''
  try {
    await onImport(async (count) => {
      showSuccess(`Imported ${count} project${count !== 1 ? 's' : ''}`)
      await loadData()
    })
  } catch (err) {
    error = `Import failed: ${err}`
  }
}

async function loadData() {
  loading = true
  error = ''
  try {
    const [profiles, config, projectsOverview, profileAccounts] = await Promise.all([
      invoke('read_aws_config'),
      invoke('get_raw_aws_config'),
      invoke('list_projects'),
      invoke('list_aws_accounts'),
    ])
    awsProfiles = profiles
    rawConfig = config
    overview = projectsOverview
    accounts = profileAccounts
    entries = projectsOverview.fileError ? [] : await invoke('list_project_entries')
  } catch (err) {
    error = `Failed to load settings: ${err}`
  } finally {
    loading = false
  }
}

// Reload this screen and the connection form after anything was saved
async function afterSave(message) {
  showSuccess(message)
  await loadData()
  onProjectsChanged?.()
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

// The profile editor, pre-filled from an aws_profile_template result
function openProfileFromTemplate(template) {
  const lines = [
    ['sso_start_url', template.ssoStartUrl],
    ['sso_region', template.ssoRegion],
    ['sso_account_id', template.ssoAccountId],
    ['sso_role_name', template.ssoRoleName],
    ['region', template.region],
  ]
    .filter(([, value]) => value)
    .map(([key, value]) => `${key} = ${value}`)
  editingProfile = { isNew: true, accountId: template.ssoAccountId }
  editName = template.name || ''
  editContent = `${lines.join('\n')}\n`
}

async function createProfileForAccount(accountId) {
  error = ''
  try {
    openProfileFromTemplate(await invoke('aws_profile_template', { accountId }))
  } catch (err) {
    error = `Failed to prepare AWS profile: ${err}`
  }
}

async function pickProfile(accountId, profile) {
  error = ''
  try {
    await invoke('set_account_profile', { accountId, profile })
    await afterSave(`Using ${profile} for account ${accountId}`)
  } catch (err) {
    error = `Failed to select profile: ${err}`
  }
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
    closeEditModal()
    await afterSave('Profile saved successfully')
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
    await afterSave('Profile deleted')
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
    await afterSave('Config saved successfully')
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
  // MySQL has no Automatic mode; PostgreSQL defaults to it
  if (projectEngine === 'mysql' && projectAuthMode === 'auto') projectAuthMode = 'password'
  if (projectEngine !== 'mysql' && projectAuthMode === 'password') projectAuthMode = 'auto'
}

function authModeOf(config) {
  if (config.authType === 'iam') return 'iam'
  if (config.passwordOnly === true || config.engine === 'mysql') return 'password'
  return 'auto'
}

// Top-level fields of a broken project's raw JSON that its salvaged config
// lost: absent, or empty where the raw value was not
function droppedFields(raw, config) {
  return Object.keys(raw).filter((field) => {
    const rawValue = raw[field]
    const kept = config[field]
    if (rawValue === null || rawValue === undefined) return false
    return kept === undefined || kept === null || (kept === '' && rawValue !== '')
  })
}

// What the form would lose by saving a broken entry from its salvaged config
function brokenInfo(entry, config) {
  let raw = null
  try {
    raw = JSON.parse(entry.raw)
  } catch {
    // raw is not an object we can compare; list nothing
  }
  const isObject = raw !== null && typeof raw === 'object' && !Array.isArray(raw)
  return {
    error: entry.error,
    droppedFields: isObject ? droppedFields(raw, config) : [],
    hadEnvironments: isObject && Array.isArray(raw.environments) && raw.environments.length > 0,
  }
}

// Open the project form. mode: 'new' | 'edit' | 'duplicate'. `entry` is the
// list entry, used to warn when a broken project is edited through the form.
function openProjectForm(mode, key = '', source = null, entry = null) {
  const config = source ? $state.snapshot(source) : {}
  const isNew = mode !== 'edit'
  const broken = !isNew && entry?.error ? brokenInfo($state.snapshot(entry), config) : null
  editingProject = { isNew, key: isNew ? null : key, broken }
  brokenSaveConfirmed = false
  validationErrors = []
  error = ''
  sectionAdvanced = false

  projectKey = mode === 'duplicate' ? `${key}-copy` : key
  projectName = mode === 'duplicate' ? `${config.name} (copy)` : config.name || ''
  projectBaseConfig = { ...config }
  projectRegion = config.region || 'us-east-1'
  projectDatabase = config.database || ''
  projectDatabases = config.databases ? [...config.databases] : []
  projectSecretPrefix = source ? config.secretPrefix || '' : 'rds!cluster'
  projectAuthMode = authModeOf(config)
  projectIamUsername = config.iamUsername || ''
  projectRdsType = config.rdsType || 'cluster'
  projectEngine = config.engine || 'postgres'
  projectRdsPattern = config.rdsPattern || ''
  projectBastionPattern = config.bastionPattern || ''
  projectDefaultPort = config.defaultPort ? String(config.defaultPort) : '5432'
  projectEnvironments = (config.environments || []).map((e) => ({ ...e, port: e.port == null ? '' : String(e.port) }))
  projectSecretPath = config.secretPath || ''
  projectSecretUsernameField = config.secretUsernameField || ''
  projectSecretPasswordField = config.secretPasswordField || ''
  projectMultiplexed = config.multiplexed !== false
  projectConnectionType = config.connectionType || 'rds'
  projectServiceType = config.serviceType || 'vnc'
  projectRemotePort = config.remotePort ? String(config.remotePort) : ''
  projectTargetType = config.targetType || 'ec2-direct'
  projectTargetPattern = config.targetPattern || ''
  projectEcsCluster = config.ecsCluster || ''
  projectEcsService = config.ecsService || ''
  projectSshUsername = config.sshUsername || ''
  projectSshKeyPath = config.sshKeyPath || ''
}

function closeProjectModal() {
  editingProject = null
  validationErrors = []
}

function addDatabase() {
  projectDatabases = [...projectDatabases, '']
}

function removeDatabase(index) {
  projectDatabases = projectDatabases.filter((_, i) => i !== index)
}

function updateDatabase(index, value) {
  projectDatabases = projectDatabases.map((d, i) => i === index ? value : d)
}

const OVERRIDE_KEYS = ['rdsPattern', 'region', 'targetPattern', 'ecsCluster', 'ecsService', 'bastionPattern']

function optional(value) {
  return value.trim() || null
}

// An environment as saved: trimmed, empty overrides left out
function cleanEnvironment(env) {
  const out = { ...$state.snapshot(env) }
  out.name = (out.name ?? '').trim()
  out.accountId = (out.accountId ?? '').trim()
  out.port = String(out.port ?? '').trim()
  for (const key of OVERRIDE_KEYS) {
    const value = typeof out[key] === 'string' ? out[key].trim() : out[key]
    if (value) out[key] = value
    else delete out[key]
  }
  return out
}

function parseRemotePort(value) {
  const v = value.trim()
  if (!/^\d+$/.test(v)) return null
  const port = Number.parseInt(v, 10)
  return port >= 1 && port <= 65535 ? port : null
}

// The project as the form would save it
function buildConfig() {
  const rds = projectConnectionType === 'rds'
  const service = !rds
  const base = projectBaseConfig
  let authType = 'secrets'
  let passwordOnly = null
  if (rds && projectAuthMode === 'iam') authType = 'iam'
  else if (rds && projectAuthMode === 'password' && projectEngine !== 'mysql') passwordOnly = true

  const isNew = editingProject?.isNew
  return {
    ...base,
    name: projectName.trim(),
    region: projectRegion.trim(),
    connectionType: projectConnectionType,
    database: rds ? projectDatabase.trim() : '',
    databases: rds && projectDatabases.length > 0 ? projectDatabases.filter((d) => d.trim()) : null,
    secretPrefix: rds ? projectSecretPrefix.trim() : '',
    authType,
    passwordOnly,
    iamUsername: rds ? optional(projectIamUsername) : null,
    rdsType: rds ? projectRdsType : '',
    engine: rds ? projectEngine : null,
    rdsPattern: rds ? projectRdsPattern.trim() : '',
    secretPath: rds ? optional(projectSecretPath) : null,
    secretUsernameField: rds ? optional(projectSecretUsernameField) : null,
    secretPasswordField: rds ? optional(projectSecretPasswordField) : null,
    // Absent means on; keep an explicit true as it was written
    multiplexed: projectMultiplexed ? (base.multiplexed === true ? true : null) : false,
    bastionPattern: optional(projectBastionPattern),
    // Legacy profile matching: fixed for new projects, kept as stored otherwise
    profileFilter: isNew ? HIDE_FROM_LEGACY_FILTER : (base.profileFilter ?? null),
    envPortMapping: isNew ? {} : (base.envPortMapping ?? {}),
    defaultPort: projectDefaultPort.trim(),
    environments: projectEnvironments.length > 0 ? projectEnvironments.map(cleanEnvironment) : null,
    serviceType: service ? projectServiceType : null,
    remotePort: service ? parseRemotePort(projectRemotePort) : null,
    targetType: service ? projectTargetType : null,
    targetPattern: service && projectTargetType !== 'ecs-bastion' ? optional(projectTargetPattern) : null,
    ecsCluster: service && projectTargetType === 'ecs-bastion' ? optional(projectEcsCluster) : null,
    ecsService: service && projectTargetType === 'ecs-bastion' ? optional(projectEcsService) : null,
    sshUsername: service && projectServiceType === 'ssh' ? optional(projectSshUsername) : null,
    sshKeyPath: service && projectServiceType === 'ssh' ? optional(projectSshKeyPath) : null,
  }
}

// Checks the backend validator cannot make (the key is not part of the config)
const localErrors = $derived.by(() => {
  if (!editingProject) return []
  const errors = []
  if (editingProject.isNew) {
    const key = projectKey.trim()
    if (!key) errors.push({ field: 'key', message: 'Project key is required' })
    else if (!NEW_KEY_PATTERN.test(key)) errors.push({ field: 'key', message: 'Use lowercase letters, digits and hyphens' })
    else if (entries.some((e) => e.key === key)) errors.push({ field: 'key', message: 'A project with this key already exists' })
  }
  if (projectConnectionType === 'service' && projectRemotePort.trim() && parseRemotePort(projectRemotePort) === null) {
    errors.push({ field: 'remotePort', message: 'Remote port must be a number from 1 to 65535' })
  }
  // Without environments a non-legacy project matches no profile and can never connect
  if (!projectIsLegacy && projectEnvironments.length === 0) {
    errors.push({ field: 'environments', message: 'Add at least one environment' })
  }
  return errors
})

// Save needs an explicit OK when it would drop fields of a broken project
const brokenSaveBlocked = $derived(!!editingProject?.broken && !brokenSaveConfirmed)

const formErrors = $derived.by(() => {
  const local = new Set(localErrors.map((e) => e.field))
  return [...localErrors, ...validationErrors.filter((e) => !local.has(e.field))]
})

const environmentErrors = $derived(formErrors.filter((e) => e.field.startsWith('environments[')))
const hasEnvironmentsError = $derived(hasErrorIn(['environments']))

// Fields the form currently shows an input for
const visibleFields = $derived.by(() => {
  const fields = new Set(['key', 'name', 'region', 'defaultPort', 'connectionType'])
  const add = (...names) => { for (const n of names) fields.add(n) }
  if (projectConnectionType === 'rds') {
    add('database', 'databases', 'rdsType', 'engine', 'rdsPattern', 'authType', 'passwordOnly')
    add('secretPath', 'secretUsernameField', 'secretPasswordField')
    if (projectAuthMode !== 'iam') add('secretPrefix')
    if (projectAuthMode !== 'password') add('iamUsername')
  } else {
    add('serviceType', 'remotePort', 'targetType')
    if (projectTargetType === 'ecs-bastion') add('ecsCluster', 'ecsService')
    else add('targetPattern')
    if (projectServiceType === 'ssh') add('sshUsername', 'sshKeyPath')
  }
  if (projectConnectionType === 'rds' || projectTargetType !== 'ec2-direct') add('bastionPattern')
  return fields
})

function isPlaced(field) {
  return visibleFields.has(field)
    || field === 'environments'
    || field.startsWith('environments[')
    || (projectConnectionType === 'rds' && field.startsWith('databases['))
    || (projectIsLegacy && field.startsWith('envPortMapping.'))
}

// Errors with no input to show them next to
const unplacedErrors = $derived(formErrors.filter((e) => !isPlaced(e.field)))

function fieldError(field) {
  return formErrors.filter((e) => e.field === field).map((e) => e.message).join('; ')
}

function hasErrorIn(fields) {
  return formErrors.some((e) => fields.some((f) => e.field === f || e.field.startsWith(`${f}[`) || e.field.startsWith(`${f}.`)))
}

const GENERAL_FIELDS = ['key', 'name', 'region', 'defaultPort', 'connectionType']
const DATABASE_FIELDS = ['database', 'databases', 'authType', 'passwordOnly', 'iamUsername', 'secretPrefix', 'rdsType', 'engine', 'rdsPattern']
const SERVICE_FIELDS = ['serviceType', 'remotePort', 'targetType', 'targetPattern', 'ecsCluster', 'ecsService', 'sshUsername', 'sshKeyPath']
const ADVANCED_FIELDS = ['secretPath', 'secretUsernameField', 'secretPasswordField', 'bastionPattern']

// A section with an error stays open so the error is visible
const environmentsOpen = $derived(sectionEnvironments || hasEnvironmentsError)
const advancedOpen = $derived(sectionAdvanced || hasErrorIn(ADVANCED_FIELDS))

// Validate 300 ms after the last change to the form
$effect(() => {
  if (!editingProject) return
  const config = buildConfig()
  const seq = ++validationSeq
  return safeTimeout(async () => {
    try {
      const errors = await invoke('validate_project', { config })
      if (seq === validationSeq) validationErrors = errors
    } catch (err) {
      if (seq === validationSeq) validationErrors = [{ field: '', message: `${err}` }]
    }
  }, VALIDATE_DELAY_MS)
})

async function saveProject() {
  if (localErrors.length > 0 || brokenSaveBlocked) return
  const key = editingProject.isNew ? projectKey.trim() : editingProject.key
  const config = buildConfig()

  saving = true
  error = ''
  try {
    await invoke('save_project_config', { key, config })
    closeProjectModal()
    await afterSave('Project saved')
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
    await afterSave('Project deleted')
  } catch (err) {
    error = `Failed to delete project: ${err}`
  } finally {
    saving = false
  }
}

// ---- Single-project JSON editor ----

function openJsonEditor(entry) {
  jsonEditKey = entry.key
  jsonEditError = ''
  try {
    jsonEditText = JSON.stringify(JSON.parse(entry.raw), null, 2)
  } catch {
    jsonEditText = entry.raw ?? ''
  }
}

function closeJsonEditor() {
  jsonEditKey = null
  jsonEditText = ''
  jsonEditError = ''
}

async function saveProjectJson() {
  try {
    JSON.parse(jsonEditText)
  } catch (err) {
    jsonEditError = `Not valid JSON: ${err.message}`
    return
  }

  const key = jsonEditKey
  saving = true
  jsonEditError = ''
  try {
    await invoke('save_project_raw', { key, json: jsonEditText })
    await afterSave('Project saved')
    const entry = entries.find((e) => e.key === key)
    if (entry?.error) {
      jsonEditError = `Saved, but the project still can't be read: ${entry.error}`
    } else {
      closeJsonEditor()
    }
  } catch (err) {
    jsonEditError = `Failed to save project: ${err}`
  } finally {
    saving = false
  }
}

// ---- Keyboard handlers ----

// Escape closes the topmost dialog, then Settings itself
function closeTopmost() {
  if (editingProfile) closeEditModal()
  else if (jsonEditKey) closeJsonEditor()
  else if (editingProject) closeProjectModal()
  else return false
  return true
}

function handleOverlayKeydown(e) {
  if (e.key === 'Escape' && !closeTopmost()) onClose()
}

function handleEditOverlayKeydown(e) {
  if (e.key === 'Escape') {
    e.stopPropagation()
    closeTopmost()
  }
}

function handleDialogKeydown(e) {
  e.stopPropagation()
  if (e.key === 'Escape') closeTopmost()
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
      <button
        class="tab"
        class:active={activeTab === 'appearance'}
        onclick={() => activeTab = 'appearance'}
      >
        Appearance
      </button>
      <button
        class="tab"
        class:active={activeTab === 'about'}
        onclick={() => activeTab = 'about'}
      >
        About
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
            <span class="profiles-path">~/.connection-app/projects.json</span>
            <div class="header-actions">
              {#if onImport}
                <button class="btn-add" onclick={startImport} disabled={!!overview.fileError}>Import...</button>
              {/if}
            <button class="btn-add" onclick={() => openProjectForm('new')} disabled={!!overview.fileError}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
              </svg>
              Add Project
            </button>
            </div>
          </div>

          {#if needsMigration && onOpenMigration}
            <div class="migration-banner" role="status">
              <span>Config needs updating: some projects still use AWS profile names.</span>
              <button type="button" class="btn-add-small" onclick={onOpenMigration}>Update to environments</button>
            </div>
          {/if}

          {#if overview.fileError}
            <ProjectFileRepair
              fileError={overview.fileError}
              backups={overview.backups}
              {invoke}
              onRepaired={afterSave}
            />
          {:else if entries.length === 0}
            <div class="empty-state">
              <p>No projects configured</p>
              <p class="hint">Click "Add Project" to get started</p>
            </div>
          {:else}
            <div class="profiles-list">
              {#each entries as entry (entry.key)}
                {@const config = entry.config}
                {@const summary = summaryByKey.get(entry.key)}
                {@const name = config?.name || entry.key}
                <div class="profile-card" class:broken={!!entry.error}>
                  <div class="profile-header">
                    <div class="project-title">
                      <span class="profile-name">{name}</span>
                      {#if entry.error}
                        <span class="badge badge-broken">Broken</span>
                      {:else if entry.problems.length > 0}
                        <span class="badge badge-invalid">{entry.problems.length} {entry.problems.length === 1 ? 'problem' : 'problems'}</span>
                      {/if}
                      {#if summary && !summary.reachable && !entry.error}
                        <span class="badge badge-no-access">No AWS access</span>
                      {/if}
                    </div>
                    <div class="profile-actions">
                      {#if config}
                        <button class="btn-icon" onclick={() => openProjectForm('edit', entry.key, config, entry)} aria-label="Edit {name}" title="Edit">
                          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                            <path d="M10.5 1.5l2 2-8 8H2.5v-2l8-8z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                          </svg>
                        </button>
                      {/if}
                      {#if entry.error}
                        <button class="btn-icon" onclick={() => openJsonEditor(entry)} aria-label="Edit JSON of {name}" title="Edit JSON">
                          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                            <path d="M5 2.5C3.5 2.5 3.5 3.5 3.5 5S2 7 2 7s1.5 0 1.5 2 0 2 1.5 2M9 2.5c1.5 0 1.5 1 1.5 2.5S12 7 12 7s-1.5 0-1.5 2 0 2-1.5 2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
                          </svg>
                        </button>
                      {:else if config}
                        <button class="btn-icon" onclick={() => openProjectForm('duplicate', entry.key, config)} aria-label="Duplicate {name}" title="Duplicate">
                          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                            <rect x="4.5" y="4.5" width="7.5" height="7.5" rx="1.2" stroke="currentColor" stroke-width="1.4"/>
                            <path d="M9.5 4.5V3a1 1 0 00-1-1H3a1 1 0 00-1 1v5.5a1 1 0 001 1h1.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
                          </svg>
                        </button>
                      {/if}
                      <button class="btn-icon delete" onclick={() => requestDeleteProject(entry.key)} aria-label="Delete {name}" title="Delete">
                        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                          <path d="M2 4h10M5 4V2.5a.5.5 0 01.5-.5h3a.5.5 0 01.5.5V4M11 4v8a1 1 0 01-1 1H4a1 1 0 01-1-1V4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                      </button>
                    </div>
                  </div>
                  {#if deleteConfirmProjectKey === entry.key}
                    <div class="inline-confirm">
                      <span>Delete "{name}"?</span>
                      <div class="inline-confirm-actions">
                        <button class="btn-inline-confirm" onclick={confirmDeleteProject}>Delete</button>
                        <button class="btn-inline-cancel" onclick={cancelDeleteProject}>Cancel</button>
                      </div>
                    </div>
                  {:else}
                    {#if entry.error}
                      <p class="entry-error">{entry.error}</p>
                    {/if}
                    {#if entry.problems.length > 0}
                      <ul class="problem-list">
                        {#each entry.problems as problem}
                          <li><span class="problem-field">{problem.field}</span> {problem.message}</li>
                        {/each}
                      </ul>
                    {/if}
                    {#if config}
                      <div class="profile-details">
                        <span class="detail">{config.region}</span>
                        {#if (config.connectionType || 'rds') === 'rds'}
                          <span class="detail">{config.rdsType}</span>
                          <span class="detail">{config.engine || 'postgres'}</span>
                          <span class="detail">{config.database}</span>
                        {:else}
                          <span class="detail">{(config.serviceType || 'custom').toUpperCase()}</span>
                          <span class="detail">{config.targetType || 'ec2-direct'}</span>
                          {#if config.serviceType === 'ssh' && config.sshUsername}
                            <span class="detail">{config.sshUsername}@</span>
                          {/if}
                          {#if config.remotePort}
                            <span class="detail">port {config.remotePort}</span>
                          {/if}
                        {/if}
                        {#if config.environments?.length > 0}
                          <span class="detail">{config.environments.map((e) => e.name).join(' · ')}</span>
                        {:else}
                          <span class="detail">legacy: AWS profile names</span>
                        {/if}
                      </div>
                    {/if}
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
      {:else if activeTab === 'raw'}
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
      {:else if activeTab === 'appearance'}
        <div class="appearance-tab">
          <ThemeSwitcher {scheme} {onSchemeChange} />
          {#if scheme !== 'light'}
            <div class="theme-section-label">Dark Themes</div>
            <div class="theme-grid">
              {#each darkThemeNames as key}
                {@const theme = themes[key]}
                <button
                  class="theme-preview-card"
                  class:selected={currentTheme === key}
                  onclick={() => onThemeChange?.(key)}
                >
                  <div class="preview-mockup" style="background: {theme.vars['--bg-primary']}">
                    <div class="preview-header" style="background: linear-gradient(90deg, {theme.vars['--accent-primary']}, {theme.vars['--accent-secondary']})"></div>
                    <div class="preview-body">
                      <div class="preview-line" style="background: {theme.vars['--text-primary']}; width: 70%"></div>
                      <div class="preview-line" style="background: {theme.vars['--text-secondary']}; width: 50%"></div>
                      <div class="preview-btn" style="background: {theme.vars['--bg-button-gradient']}"></div>
                    </div>
                  </div>
                  <span class="theme-card-name">{theme.name}</span>
                  {#if currentTheme === key}
                    <div class="theme-check">
                      <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                        <path d="M2 6l3 3 5-5" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                      </svg>
                    </div>
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
          {#if scheme !== 'dark'}
            <div class="theme-section-label">Light Themes</div>
            <div class="theme-grid">
              {#each lightThemeNames as key}
                {@const theme = themes[key]}
                <button
                  class="theme-preview-card"
                  class:selected={currentLightTheme === key}
                  onclick={() => onLightThemeChange?.(key)}
                >
                  <div class="preview-mockup" style="background: {theme.vars['--bg-primary']}">
                    <div class="preview-header" style="background: linear-gradient(90deg, {theme.vars['--accent-primary']}, {theme.vars['--accent-secondary']})"></div>
                    <div class="preview-body">
                      <div class="preview-line" style="background: {theme.vars['--text-primary']}; width: 70%"></div>
                      <div class="preview-line" style="background: {theme.vars['--text-secondary']}; width: 50%"></div>
                      <div class="preview-btn" style="background: {theme.vars['--bg-button-gradient']}"></div>
                    </div>
                  </div>
                  <span class="theme-card-name">{theme.name}</span>
                  {#if currentLightTheme === key}
                    <div class="theme-check">
                      <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                        <path d="M2 6l3 3 5-5" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                      </svg>
                    </div>
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
          {#if scheme === 'system'}
            <p class="scheme-hint">Your dark and light themes are used based on your system's appearance setting.</p>
          {/if}
        </div>
      {:else if activeTab === 'about'}
        <div class="about-tab">
          <div class="about-app-name">ConnectionApp</div>
          {#if appVersion}
            <div class="about-version">v{appVersion}</div>
          {/if}
          <p class="about-description">Secure tunneling through AWS Systems Manager for RDS databases and services.</p>
          <div class="about-links">
            <button class="about-link" onclick={() => invoke('open_url', { url: 'https://github.com/yarka-guru/connection_app' })}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M8 1C4.13 1 1 4.13 1 8c0 3.1 2.01 5.73 4.79 6.65.35.06.48-.15.48-.34 0-.17-.01-.71-.01-1.29-1.76.33-2.2-.43-2.34-.82-.08-.2-.42-.82-.71-.98-.24-.13-.59-.46-.01-.47.55-.01.94.5 1.07.71.63 1.05 1.63.76 2.03.57.06-.45.24-.76.44-.93-1.55-.18-3.18-.78-3.18-3.45 0-.76.27-1.39.71-1.88-.07-.17-.31-.89.07-1.85 0 0 .58-.19 1.9.71a6.5 6.5 0 011.74-.24c.59 0 1.18.08 1.74.24 1.32-.9 1.9-.71 1.9-.71.38.96.14 1.68.07 1.85.44.49.71 1.11.71 1.88 0 2.68-1.63 3.27-3.19 3.44.25.22.47.64.47 1.29 0 .93-.01 1.68-.01 1.91 0 .19.13.41.48.34A7.01 7.01 0 0015 8c0-3.87-3.13-7-7-7z" fill="currentColor"/>
              </svg>
              GitHub Repository
            </button>
            <button class="about-link" onclick={() => invoke('open_url', { url: 'https://github.com/yarka-guru/connection_app/blob/main/PRIVACY.md' })}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M8 1.5a6.5 6.5 0 100 13 6.5 6.5 0 000-13zM8 5a.75.75 0 110 1.5A.75.75 0 018 5zm1 6.5H7v-1h.5V8H7V7h1.5v3.5H9v1z" fill="currentColor"/>
              </svg>
              Privacy Policy
            </button>
            <button class="about-link" onclick={() => invoke('open_url', { url: 'https://github.com/yarka-guru/connection_app/blob/main/LICENSE' })}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M4 2h8a1 1 0 011 1v10a1 1 0 01-1 1H4a1 1 0 01-1-1V3a1 1 0 011-1zm1 3v1h6V5H5zm0 2.5v1h6v-1H5zm0 2.5v1h4v-1H5z" fill="currentColor"/>
              </svg>
              MIT License
            </button>
            <button class="about-link" onclick={() => invoke('open_url', { url: 'https://github.com/yarka-guru/connection_app/blob/main/THIRD_PARTY_LICENSES.md' })}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M3 2h7l3 3v8a1 1 0 01-1 1H3a1 1 0 01-1-1V3a1 1 0 011-1zm6 0v3h3M5 8h6M5 10.5h4" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>
              </svg>
              Third-Party Licenses
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>

  {#snippet fieldErrorText(field)}
    {@const message = fieldError(field)}
    {#if message}<span class="field-error">{message}</span>{/if}
  {/snippet}

  <!-- Edit Project Modal -->
  {#if editingProject}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="edit-modal-overlay" onclick={closeProjectModal} onkeydown={handleEditOverlayKeydown}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="edit-modal project-modal" onclick={(e) => e.stopPropagation()} onkeydown={handleDialogKeydown} use:trapFocus role="dialog" tabindex="-1" aria-label={editingProject.isNew ? 'Add project' : 'Edit project'}>
        <h3>{editingProject.isNew ? 'Add Project' : 'Edit Project'}</h3>

        {#if error}
          <div class="message error">{error}</div>
        {/if}

        {#if editingProject.broken}
          <div class="broken-warning" role="alert">
            <p class="broken-warning-title">This project can't be read as it is</p>
            <p class="broken-warning-error">{editingProject.broken.error}</p>
            {#if editingProject.broken.droppedFields.length > 0}
              <p>Saving from this form removes these fields, which could not be read:</p>
              <ul>
                {#each editingProject.broken.droppedFields as field (field)}
                  <li><code>{field}</code></li>
                {/each}
              </ul>
            {:else}
              <p>Saving from this form rewrites the project from what could be read.</p>
            {/if}
            <p>To keep everything, cancel and use <strong>Edit JSON</strong> instead.</p>
            <label class="broken-confirm">
              <input type="checkbox" bind:checked={brokenSaveConfirmed} />
              I understand the fields listed above will be removed
            </label>
          </div>
        {/if}

        <div class="project-form-scroll">
          <!-- General Section -->
          <div class="form-section">
            <button type="button" class="section-header" onclick={() => sectionGeneral = !sectionGeneral} aria-expanded={sectionGeneral || hasErrorIn(GENERAL_FIELDS)}>
              <svg class="section-chevron" class:open={sectionGeneral || hasErrorIn(GENERAL_FIELDS)} width="14" height="14" viewBox="0 0 14 14" fill="none">
                <path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              <span class="section-title">General</span>
            </button>
            {#if sectionGeneral || hasErrorIn(GENERAL_FIELDS)}
              <div class="section-body">
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
                  {@render fieldErrorText('key')}
                </div>

                <div class="form-group">
                  <label for="project-name">Name</label>
                  <input id="project-name" type="text" bind:value={projectName} placeholder="My Project" />
                  {@render fieldErrorText('name')}
                </div>

                <div class="form-row">
                  <div class="form-group">
                    <label for="project-region">Region</label>
                    <input id="project-region" type="text" bind:value={projectRegion} placeholder="us-east-1" />
                    {@render fieldErrorText('region')}
                  </div>
                  <div class="form-group">
                    <label for="project-default-port">Default Port</label>
                    <input id="project-default-port" type="text" inputmode="numeric" bind:value={projectDefaultPort} placeholder={projectConnectionType === 'rds' ? '5432' : '5900'} />
                    <span class="field-hint">First port suggested for a new environment</span>
                    {@render fieldErrorText('defaultPort')}
                  </div>
                </div>

                <div class="form-group">
                  <span class="group-label" id="project-connection-type-label">Connection Type</span>
                  <div class="type-toggle" role="group" aria-labelledby="project-connection-type-label">
                    <button
                      type="button"
                      class="type-btn"
                      class:active={projectConnectionType === 'rds'}
                      aria-pressed={projectConnectionType === 'rds'}
                      onclick={() => { projectConnectionType = 'rds'; if (projectDefaultPort === '5900' || projectDefaultPort === '3389') { projectDefaultPort = projectEngine === 'mysql' ? '3306' : '5432' } }}
                    >RDS Database</button>
                    <button
                      type="button"
                      class="type-btn"
                      class:active={projectConnectionType === 'service'}
                      aria-pressed={projectConnectionType === 'service'}
                      onclick={() => { projectConnectionType = 'service'; if (projectDefaultPort === '5432' || projectDefaultPort === '3306') { projectDefaultPort = '5900' } }}
                    >VNC/RDP Service</button>
                  </div>
                  {@render fieldErrorText('connectionType')}
                </div>
              </div>
            {/if}
          </div>

          <!-- Database Section (RDS only) -->
          {#if projectConnectionType === 'rds'}
            {@const databaseOpen = sectionDatabase || hasErrorIn(DATABASE_FIELDS)}
            <div class="form-section">
              <button type="button" class="section-header" onclick={() => sectionDatabase = !sectionDatabase} aria-expanded={databaseOpen}>
                <svg class="section-chevron" class:open={databaseOpen} width="14" height="14" viewBox="0 0 14 14" fill="none">
                  <path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
                <span class="section-title">Database</span>
                <span class="section-badge">RDS</span>
              </button>
              {#if databaseOpen}
                <div class="section-body">
                  <div class="form-row">
                    <div class="form-group">
                      <label for="project-rds-type">RDS Type</label>
                      <select id="project-rds-type" bind:value={projectRdsType}>
                        <option value="cluster">Cluster (Aurora)</option>
                        <option value="instance">Instance</option>
                      </select>
                      {@render fieldErrorText('rdsType')}
                    </div>
                    <div class="form-group">
                      <label for="project-engine">Engine</label>
                      <select id="project-engine" value={projectEngine} onchange={handleEngineChange}>
                        <option value="postgres">PostgreSQL</option>
                        <option value="mysql">MySQL</option>
                      </select>
                      {@render fieldErrorText('engine')}
                    </div>
                  </div>

                  <div class="form-group">
                    <label for="project-rds-pattern">RDS Pattern</label>
                    <input id="project-rds-pattern" type="text" bind:value={projectRdsPattern} placeholder="-rds-aurora" />
                    <span class="field-hint">Substring to match in cluster/instance name</span>
                    {@render fieldErrorText('rdsPattern')}
                  </div>

                  <div class="form-group">
                    <label for="project-database">Database</label>
                    <input id="project-database" type="text" bind:value={projectDatabase} placeholder="mydb" />
                    {@render fieldErrorText('database')}
                  </div>

                  <div class="form-group databases-group">
                    <span class="group-label">
                      Databases
                      <span class="label-hint">(optional, for multi-database selection)</span>
                    </span>
                    {#if projectDatabases.length > 0}
                      <div class="databases-list">
                        {#each projectDatabases as db, i}
                          <div class="database-entry">
                            <input
                              type="text"
                              value={db}
                              oninput={(e) => updateDatabase(i, e.target.value)}
                              placeholder="database name"
                              aria-label="Database {i + 1}"
                            />
                            <button type="button" class="btn-icon-remove" onclick={() => removeDatabase(i)} title="Remove database" aria-label="Remove database {i + 1}">
                              <svg width="14" height="14" viewBox="0 0 14 14" fill="none"><path d="M3 3l8 8M11 3l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
                            </button>
                          </div>
                          {@render fieldErrorText(`databases[${i}]`)}
                        {/each}
                      </div>
                    {/if}
                    {@render fieldErrorText('databases')}
                    <button type="button" class="btn-add-item" onclick={addDatabase}>+ Add Database</button>
                  </div>

                  <div class="form-group">
                    <label for="project-auth-mode">Authentication</label>
                    <select id="project-auth-mode" bind:value={projectAuthMode}>
                      {#if projectEngine === 'mysql'}
                        <option value="iam">IAM</option>
                        <option value="password">Password</option>
                      {:else}
                        <option value="auto">Automatic</option>
                        <option value="iam">IAM only</option>
                        <option value="password">Password only</option>
                      {/if}
                    </select>
                    {#if projectEngine !== 'mysql'}
                      <span class="field-hint">{AUTH_HINTS[projectAuthMode]}</span>
                    {/if}
                    {@render fieldErrorText('authType')}
                    {@render fieldErrorText('passwordOnly')}
                  </div>

                  <div class="form-row">
                    {#if projectAuthMode !== 'iam'}
                      <div class="form-group">
                        <label for="project-secret-prefix">Secret Prefix</label>
                        <input id="project-secret-prefix" type="text" bind:value={projectSecretPrefix} placeholder="rds!cluster" />
                        <span class="field-hint">Prefix to find credentials in Secrets Manager</span>
                        {@render fieldErrorText('secretPrefix')}
                      </div>
                    {/if}
                    {#if projectAuthMode !== 'password'}
                      <div class="form-group">
                        <label for="project-iam-username">IAM DB User</label>
                        <input id="project-iam-username" type="text" bind:value={projectIamUsername} placeholder="db_user" />
                        <span class="field-hint">
                          {projectAuthMode === 'auto' ? 'Used only when no secret is configured' : 'Database role granted rds_iam; the token is valid for 15 minutes'}
                        </span>
                        {@render fieldErrorText('iamUsername')}
                      </div>
                    {/if}
                  </div>
                </div>
              {/if}
            </div>
          {/if}

          <!-- Service Section (Service only) -->
          {#if projectConnectionType === 'service'}
            {@const serviceOpen = sectionService || hasErrorIn(SERVICE_FIELDS)}
            <div class="form-section">
              <button type="button" class="section-header" onclick={() => sectionService = !sectionService} aria-expanded={serviceOpen}>
                <svg class="section-chevron" class:open={serviceOpen} width="14" height="14" viewBox="0 0 14 14" fill="none">
                  <path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
                <span class="section-title">Service</span>
                <span class="section-badge">{(projectServiceType || 'vnc').toUpperCase()}</span>
              </button>
              {#if serviceOpen}
                <div class="section-body">
                  <div class="form-row">
                    <div class="form-group">
                      <label for="project-service-type">Service Type</label>
                      <select id="project-service-type" bind:value={projectServiceType}>
                        <option value="vnc">VNC</option>
                        <option value="rdp">RDP</option>
                        <option value="ssh">SSH</option>
                        <option value="custom">Custom</option>
                      </select>
                      {@render fieldErrorText('serviceType')}
                    </div>
                    <div class="form-group">
                      <label for="project-remote-port">Remote Port</label>
                      <input id="project-remote-port" type="text" inputmode="numeric" bind:value={projectRemotePort} placeholder={projectServiceType === 'rdp' ? '3389' : '5900'} />
                      {@render fieldErrorText('remotePort')}
                    </div>
                  </div>

                  <div class="form-group">
                    <label for="project-target-type">Target Type</label>
                    <select id="project-target-type" bind:value={projectTargetType}>
                      <option value="ec2-direct">EC2 Direct (SSM Agent)</option>
                      <option value="ec2-bastion">EC2 via Bastion</option>
                      <option value="ecs-bastion">ECS via Bastion</option>
                    </select>
                    <span class="field-hint">
                      {#if projectTargetType === 'ec2-direct'}
                        Connects directly to EC2 instance (requires SSM Agent)
                      {:else if projectTargetType === 'ec2-bastion'}
                        Tunnels through bastion to EC2 instance private IP
                      {:else}
                        Tunnels through bastion to ECS task private IP
                      {/if}
                    </span>
                    {@render fieldErrorText('targetType')}
                  </div>

                  {#if projectTargetType === 'ec2-direct' || projectTargetType === 'ec2-bastion'}
                    <div class="form-group">
                      <label for="project-target-pattern">EC2 Name Pattern</label>
                      <input id="project-target-pattern" type="text" bind:value={projectTargetPattern} placeholder="*my-server*" />
                      <span class="field-hint">EC2 Name tag filter (supports * and ? wildcards)</span>
                      {@render fieldErrorText('targetPattern')}
                    </div>
                  {:else}
                    <div class="form-row">
                      <div class="form-group">
                        <label for="project-ecs-cluster">ECS Cluster</label>
                        <input id="project-ecs-cluster" type="text" bind:value={projectEcsCluster} placeholder="my-cluster" />
                        {@render fieldErrorText('ecsCluster')}
                      </div>
                      <div class="form-group">
                        <label for="project-ecs-service">ECS Service</label>
                        <input id="project-ecs-service" type="text" bind:value={projectEcsService} placeholder="my-service" />
                        {@render fieldErrorText('ecsService')}
                      </div>
                    </div>
                  {/if}

                  {#if projectServiceType === 'ssh'}
                    <div class="form-row">
                      <div class="form-group">
                        <label for="project-ssh-username">SSH Username</label>
                        <input id="project-ssh-username" type="text" bind:value={projectSshUsername} placeholder="ec2-user (default)" />
                        <span class="field-hint">Remote SSH username (defaults to ec2-user)</span>
                        {@render fieldErrorText('sshUsername')}
                      </div>
                      <div class="form-group">
                        <label for="project-ssh-key-path">SSH Key Path</label>
                        <input id="project-ssh-key-path" type="text" bind:value={projectSshKeyPath} placeholder="~/.ssh/id_rsa (optional)" />
                        <span class="field-hint">Path to private key (uses SSH agent if empty)</span>
                        {@render fieldErrorText('sshKeyPath')}
                      </div>
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
          {/if}

          <!-- Environments Section -->
          <div class="form-section">
            <button type="button" class="section-header" onclick={() => sectionEnvironments = !sectionEnvironments} aria-expanded={environmentsOpen}>
              <svg class="section-chevron" class:open={environmentsOpen} width="14" height="14" viewBox="0 0 14 14" fill="none">
                <path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              <span class="section-title">Environments</span>
              {#if projectEnvironments.length > 0}
                <span class="section-badge">{projectEnvironments.length}</span>
              {/if}
            </button>
            {#if environmentsOpen}
              <div class="section-body">
                {@render fieldErrorText('environments')}
                {#if projectIsLegacy}
                  <div class="legacy-notice">
                    <p>
                      This project still finds AWS profiles by name
                      {#if projectBaseConfig.profileFilter}(profiles starting with <code>{projectBaseConfig.profileFilter}</code>){/if}.
                      Environments pick the profile by AWS account instead.
                    </p>
                    {#each Object.entries(projectBaseConfig.envPortMapping || {}) as [suffix, port]}
                      {@const mappingError = fieldError(`envPortMapping.${suffix}`)}
                      {#if mappingError}<span class="field-error">{mappingError}</span>{/if}
                    {/each}
                    {#if onOpenMigration}
                      <button type="button" class="btn-add-small" onclick={() => { closeProjectModal(); onOpenMigration() }}>Update to environments</button>
                    {/if}
                  </div>
                {/if}
                <EnvironmentsEditor
                  bind:environments={projectEnvironments}
                  {accounts}
                  projects={copySources}
                  connectionType={projectConnectionType}
                  defaultPort={projectDefaultPort}
                  errors={environmentErrors}
                  {chosenProfiles}
                  onCreateProfile={createProfileForAccount}
                  onPickProfile={pickProfile}
                />
              </div>
            {/if}
          </div>

          <!-- Advanced Section (collapsed by default) -->
          <div class="form-section">
            <button type="button" class="section-header" onclick={() => sectionAdvanced = !sectionAdvanced} aria-expanded={advancedOpen}>
              <svg class="section-chevron" class:open={advancedOpen} width="14" height="14" viewBox="0 0 14 14" fill="none">
                <path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              <span class="section-title">Advanced</span>
            </button>
            {#if advancedOpen}
              <div class="section-body">
                {#if projectConnectionType === 'rds'}
                  <div class="form-group">
                    <label for="project-secret-path">Secret Path</label>
                    <input id="project-secret-path" type="text" bind:value={projectSecretPath} placeholder="(optional) exact secret name or ARN" />
                    <span class="field-hint">Used instead of searching by Secret Prefix</span>
                    {@render fieldErrorText('secretPath')}
                  </div>
                  <div class="form-row">
                    <div class="form-group">
                      <label for="project-secret-username-field">Username Field</label>
                      <input id="project-secret-username-field" type="text" bind:value={projectSecretUsernameField} placeholder="username" />
                      {@render fieldErrorText('secretUsernameField')}
                    </div>
                    <div class="form-group">
                      <label for="project-secret-password-field">Password Field</label>
                      <input id="project-secret-password-field" type="text" bind:value={projectSecretPasswordField} placeholder="password" />
                      {@render fieldErrorText('secretPasswordField')}
                    </div>
                  </div>
                {/if}

                {#if projectConnectionType === 'rds' || projectTargetType !== 'ec2-direct'}
                  <div class="form-group">
                    <label for="project-bastion-pattern">Bastion Name Pattern</label>
                    <input id="project-bastion-pattern" type="text" bind:value={projectBastionPattern} placeholder="*bastion* (default)" />
                    <span class="field-hint">EC2 Name tag filter for bastion instances (supports * wildcards)</span>
                    {@render fieldErrorText('bastionPattern')}
                  </div>
                {/if}

                <div class="form-group checkbox-group">
                  <label class="checkbox-label">
                    <input type="checkbox" bind:checked={projectMultiplexed} />
                    Multiplexed tunnel
                  </label>
                  <span class="field-hint">Several connections share one tunnel. Turn off only if a client misbehaves.</span>
                </div>
              </div>
            {/if}
          </div>
        </div>

        {#if unplacedErrors.length > 0}
          <ul class="form-errors">
            {#each unplacedErrors as e}
              <li>{e.message}</li>
            {/each}
          </ul>
        {/if}

        <div class="edit-actions">
          <button class="btn-cancel" onclick={closeProjectModal}>Cancel</button>
          <button class="btn-save" onclick={saveProject} disabled={saving || formErrors.length > 0 || brokenSaveBlocked}>
            {saving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Single-project JSON editor -->
  {#if jsonEditKey}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="edit-modal-overlay" onclick={closeJsonEditor} onkeydown={handleEditOverlayKeydown}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="edit-modal project-modal" onclick={(e) => e.stopPropagation()} onkeydown={handleDialogKeydown} use:trapFocus role="dialog" tabindex="-1" aria-label="Edit project JSON">
        <h3>Edit JSON: {jsonEditKey}</h3>
        <div class="form-group">
          <label for="project-json">Project</label>
          <textarea id="project-json" class="json-editor" bind:value={jsonEditText} spellcheck="false"></textarea>
          {#if jsonEditError}<span class="field-error">{jsonEditError}</span>{/if}
        </div>
        <div class="edit-actions">
          <button class="btn-cancel" onclick={closeJsonEditor}>Cancel</button>
          <button class="btn-save" onclick={saveProjectJson} disabled={saving}>
            {saving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Edit AWS Profile Modal (last, so it stacks above the project form) -->
  {#if editingProfile}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="edit-modal-overlay" onclick={closeEditModal} onkeydown={handleEditOverlayKeydown}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="edit-modal" onclick={(e) => e.stopPropagation()} onkeydown={handleDialogKeydown} use:trapFocus role="dialog" tabindex="-1" aria-label={editingProfile.isNew ? 'Add profile' : 'Edit profile'}>
        <h3>{editingProfile.isNew ? 'Add Profile' : 'Edit Profile'}</h3>

        {#if error}
          <div class="message error">{error}</div>
        {/if}
        {#if editingProfile.accountId}
          <p class="template-note">Pre-filled for account {editingProfile.accountId} from your existing SSO settings. Name the profile, check the settings, then save.</p>
        {/if}

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
</div>

<style>
  .settings-modal {
    position: fixed;
    inset: 0;
    background: var(--overlay-bg);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 24px;
  }

  .modal-content {
    background: var(--bg-card);
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
    color: var(--text-primary);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
    transition: background-color 0.2s, color 0.2s;
  }

  .close-btn:hover {
    background: rgba(var(--glass-rgb), 0.1);
    color: var(--text-primary);
  }

  .tabs {
    display: flex;
    gap: 4px;
    background: rgba(var(--glass-rgb), 0.03);
    padding: 4px;
    border-radius: 10px;
    margin-bottom: 16px;
  }

  .tab {
    flex: 1;
    padding: 10px 16px;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .tab:hover {
    color: var(--text-hover);
    background: var(--glass-bg-hover);
  }

  .tab.active {
    background: rgba(var(--accent-primary-rgb), 0.15);
    color: var(--accent-primary-light);
  }

  .message {
    padding: 10px 14px;
    border-radius: 8px;
    font-size: 0.875rem;
    margin-bottom: 12px;
  }

  .message.error {
    background: rgba(var(--color-error-rgb), 0.1);
    color: var(--color-error-soft);
    border: 1px solid rgba(var(--color-error-rgb), 0.2);
  }

  .message.success {
    background: rgba(var(--accent-secondary-rgb), 0.1);
    color: var(--color-success-soft);
    border: 1px solid rgba(var(--accent-secondary-rgb), 0.2);
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
    color: var(--text-secondary);
  }

  .profiles-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .header-actions {
    display: flex;
    gap: 8px;
  }

  .migration-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 16px;
    padding: 10px 12px;
    font-size: 0.8rem;
    color: var(--text-hover);
    background: rgba(var(--accent-primary-rgb), 0.08);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.2);
    border-radius: 8px;
  }

  .profiles-path {
    font-size: 0.75rem;
    color: var(--text-inactive);
    font-family: ui-monospace, monospace;
  }

  .btn-add {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--accent-primary-light);
    background: rgba(var(--accent-primary-rgb), 0.1);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.2);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-add:hover {
    background: rgba(var(--accent-primary-rgb), 0.15);
  }

  .btn-add:active:not(:disabled) {
    transform: var(--press-scale);
  }

  .btn-add:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .empty-state {
    text-align: center;
    padding: 40px 20px;
    color: var(--text-secondary);
  }

  .empty-state p {
    margin: 0 0 8px;
  }

  .empty-state .hint {
    font-size: 0.875rem;
    color: var(--text-inactive);
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
    color: var(--text-primary);
  }

  .profile-card.broken {
    border-color: rgba(var(--color-error-rgb), 0.35);
  }

  .project-title {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .badge {
    font-size: 0.65rem;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: 4px;
    white-space: nowrap;
  }

  .badge-broken {
    color: var(--color-error-soft);
    background: rgba(var(--color-error-rgb), 0.15);
  }

  .badge-invalid {
    color: var(--color-warning, #e0a63a);
    background: rgba(var(--color-warning-rgb, 228, 185, 88), 0.15);
  }

  .badge-no-access {
    color: #9e9ea7;
    background: rgba(var(--glass-rgb), 0.08);
  }

  .entry-error {
    margin: 0 0 8px;
    font-size: 0.75rem;
    color: var(--color-error-soft);
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    word-break: break-word;
  }

  .problem-list {
    margin: 0 0 8px;
    padding-left: 18px;
    font-size: 0.75rem;
    color: var(--color-warning, #e0a63a);
  }

  .problem-field {
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    color: #9e9ea7;
  }

  .profile-actions {
    display: flex;
    gap: 4px;
  }

  .btn-icon {
    padding: 6px;
    background: none;
    border: none;
    color: var(--text-muted);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-icon:hover {
    background: rgba(var(--glass-rgb), 0.1);
    color: var(--text-hover);
  }

  .btn-icon.delete:hover {
    background: rgba(var(--color-error-rgb), 0.1);
    color: var(--color-error-soft);
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
    color: var(--color-error-soft);
  }

  .inline-confirm-actions {
    display: flex;
    gap: 6px;
  }

  .btn-inline-confirm {
    padding: 4px 10px;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--button-text);
    background: var(--color-error);
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-inline-confirm:hover {
    background: var(--color-error-dark);
  }

  .btn-inline-cancel {
    padding: 4px 10px;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--text-secondary);
    background: transparent;
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-inline-cancel:hover {
    background: rgba(var(--glass-rgb), 0.05);
  }

  .profile-details {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .detail {
    font-size: 0.75rem;
    color: var(--text-secondary);
    background: rgba(var(--glass-rgb), 0.05);
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
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 10px;
    padding: 14px;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    color: var(--text-primary);
    resize: none;
    outline: none;
  }

  .raw-editor:focus {
    border-color: var(--accent-primary);
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
    color: var(--button-text);
    background: var(--bg-button-gradient);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s;
  }

  .btn-save:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px var(--bg-button-gradient-shadow);
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
    background: var(--overlay-bg);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 110;
  }

  .edit-modal {
    background: var(--bg-card-inner);
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
    width: 560px;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
  }

  .project-form-scroll {
    flex: 1;
    overflow-y: auto;
    margin-bottom: 8px;
    min-height: 0;
    padding-right: 4px;
  }

  .edit-modal h3 {
    margin: 0 0 20px;
    font-size: 1.1rem;
    color: var(--text-primary);
  }

  .form-group {
    margin-bottom: 16px;
  }

  .form-group label {
    display: block;
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--text-hover);
    margin-bottom: 8px;
  }

  .form-group input,
  .form-group textarea,
  .form-group select {
    width: 100%;
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 0.875rem;
    color: var(--text-primary);
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
    border-color: var(--accent-primary);
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
    color: var(--text-inactive);
    margin-top: 4px;
  }

  .field-error {
    display: block;
    font-size: 0.72rem;
    color: var(--color-error-soft);
    margin-top: 4px;
  }

  .group-label {
    display: block;
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--text-hover);
    margin-bottom: 8px;
  }

  .form-group .checkbox-label {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 0;
    cursor: pointer;
  }

  .form-group .checkbox-label input {
    width: auto;
    margin: 0;
    accent-color: var(--accent-primary);
  }

  .legacy-notice {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    border-radius: 8px;
    background: rgba(var(--color-warning-rgb, 228, 185, 88), 0.08);
    border: 1px solid rgba(var(--color-warning-rgb, 228, 185, 88), 0.25);
  }

  .legacy-notice p {
    margin: 0;
    font-size: 0.78rem;
    line-height: 1.4;
    color: var(--text-primary);
  }

  .legacy-notice code {
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.75rem;
  }

  .form-errors {
    margin: 0 0 4px;
    padding: 8px 12px 8px 28px;
    border-radius: 8px;
    font-size: 0.75rem;
    color: var(--color-error-soft);
    background: rgba(var(--color-error-rgb), 0.08);
  }

  .json-editor {
    min-height: 280px;
  }

  .broken-warning {
    margin-bottom: 12px;
    padding: 10px 12px;
    border-radius: 8px;
    font-size: 0.78rem;
    line-height: 1.4;
    color: var(--text-primary);
    background: rgba(var(--color-error-rgb), 0.08);
    border: 1px solid rgba(var(--color-error-rgb), 0.3);
  }

  .broken-warning p {
    margin: 0 0 6px;
  }

  .broken-warning ul {
    margin: 0 0 6px;
    padding-left: 18px;
  }

  .broken-warning code,
  .broken-warning-error {
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.75rem;
  }

  .broken-warning-title {
    font-weight: 600;
    color: var(--color-error-soft);
  }

  .broken-warning-error {
    color: var(--color-error-soft);
    word-break: break-word;
  }

  .broken-confirm {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    font-weight: 500;
    cursor: pointer;
  }

  .broken-confirm input {
    margin: 0;
    accent-color: var(--color-error);
  }

  .template-note {
    margin: 0 0 16px;
    font-size: 0.78rem;
    line-height: 1.4;
    color: #9e9ea7;
  }

  .type-toggle {
    display: flex;
    gap: 4px;
    background: rgba(var(--glass-rgb), 0.03);
    padding: 3px;
    border-radius: 8px;
    border: 1px solid rgba(var(--glass-rgb), 0.1);
  }

  .type-btn {
    flex: 1;
    padding: 8px 12px;
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .type-btn:hover {
    color: var(--text-hover);
    background: var(--glass-bg-hover);
  }

  .type-btn.active {
    background: rgba(var(--accent-primary-rgb), 0.15);
    color: var(--accent-primary-light);
    font-weight: 600;
  }

  .form-row {
    display: flex;
    gap: 12px;
  }

  .form-row .form-group {
    flex: 1;
  }

  /* Collapsible form sections */
  .form-section {
    border: 1px solid rgba(var(--glass-rgb), 0.08);
    border-radius: 10px;
    margin-bottom: 10px;
    overflow: hidden;
  }

  .section-header {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    background: rgba(var(--glass-rgb), 0.03);
    border: none;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .section-header:hover {
    background: rgba(var(--glass-rgb), 0.06);
  }

  .section-chevron {
    color: var(--text-muted);
    transition: transform 0.2s;
    flex-shrink: 0;
  }

  .section-chevron.open {
    transform: rotate(90deg);
  }

  .section-title {
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: 0.02em;
  }

  .section-badge {
    font-size: 0.65rem;
    font-weight: 500;
    color: var(--accent-primary);
    background: rgba(var(--accent-primary-rgb), 0.12);
    padding: 2px 6px;
    border-radius: 4px;
    margin-left: auto;
  }

  .section-body {
    padding: 12px;
    border-top: 1px solid rgba(var(--glass-rgb), 0.06);
  }

  .btn-add-small {
    padding: 4px 10px;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--accent-primary-light);
    background: rgba(var(--accent-primary-rgb), 0.1);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.2);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-add-small:hover {
    background: rgba(var(--accent-primary-rgb), 0.15);
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
    color: var(--text-muted);
    background: none;
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-cancel:hover {
    background: rgba(var(--glass-rgb), 0.05);
    color: var(--text-hover);
  }

  .btn-cancel:active {
    transform: var(--press-scale);
  }

  .appearance-tab {
    padding: 8px 0;
  }

  .theme-section-label {
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--text-secondary);
    margin-bottom: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .theme-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
    margin-bottom: 16px;
  }

  .theme-preview-card {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 10px;
    background: rgba(var(--glass-rgb), 0.04);
    border: 2px solid rgba(var(--glass-rgb), 0.08);
    border-radius: 12px;
    cursor: pointer;
    transition: border-color 0.2s, background-color 0.2s;
  }

  .theme-preview-card:hover {
    background: rgba(var(--glass-rgb), 0.07);
    border-color: rgba(var(--glass-rgb), 0.15);
  }

  .theme-preview-card.selected {
    border-color: var(--accent-primary);
    border-width: 2px;
    background: rgba(var(--accent-primary-rgb), 0.08);
  }

  .preview-mockup {
    width: 100%;
    aspect-ratio: 4 / 3;
    border-radius: 6px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    border: 1px solid rgba(128, 128, 128, 0.15);
  }

  .preview-header {
    height: 4px;
    flex-shrink: 0;
  }

  .preview-body {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 4px;
    padding: 6px 8px;
  }

  .preview-line {
    height: 3px;
    border-radius: 2px;
    opacity: 0.8;
  }

  .preview-btn {
    height: 6px;
    width: 60%;
    border-radius: 3px;
    margin-top: 2px;
  }

  .theme-card-name {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--text-primary);
  }

  .theme-check {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 18px;
    height: 18px;
    background: var(--accent-primary);
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    color: white;
  }

  .scheme-hint {
    margin: 0 0 8px;
    font-size: 0.78rem;
    color: var(--text-muted);
    line-height: 1.4;
  }

  .databases-group {
    margin-bottom: 16px;
  }

  .databases-group .label-hint {
    font-size: 0.7rem;
    font-weight: 400;
    color: var(--text-muted);
    margin-left: 4px;
  }

  .databases-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 8px;
  }

  .database-entry {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .database-entry input {
    flex: 1;
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 6px;
    padding: 8px 10px;
    font-size: 0.8rem;
    color: var(--text-primary);
    outline: none;
  }

  .database-entry input:focus {
    border-color: var(--accent-primary);
  }

  .btn-icon-remove {
    padding: 6px;
    background: none;
    border: none;
    color: var(--text-muted);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
    flex-shrink: 0;
  }

  .btn-icon-remove:hover {
    background: rgba(var(--color-error-rgb), 0.1);
    color: var(--color-error-soft);
  }

  .btn-add-item {
    background: none;
    border: 1px dashed rgba(var(--glass-rgb), 0.15);
    border-radius: 6px;
    padding: 6px 12px;
    font-size: 0.75rem;
    color: var(--accent-primary-light);
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s;
    width: 100%;
  }

  .btn-add-item:hover {
    background: rgba(var(--accent-primary-rgb), 0.08);
    border-color: rgba(var(--accent-primary-rgb), 0.3);
  }

  .about-tab {
    padding: 24px 8px;
    text-align: center;
  }

  .about-app-name {
    font-size: 1.25rem;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .about-version {
    font-size: 0.8rem;
    color: var(--text-secondary);
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    margin-bottom: 12px;
  }

  .about-description {
    font-size: 0.875rem;
    color: var(--text-secondary);
    margin: 0 0 24px;
    line-height: 1.5;
  }

  .about-links {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 280px;
    margin: 0 auto;
  }

  .about-link {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    font-size: 0.85rem;
    font-weight: 500;
    color: var(--text-primary);
    background: rgba(var(--glass-rgb), 0.04);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 10px;
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s;
  }

  .about-link:hover {
    background: rgba(var(--glass-rgb), 0.08);
    border-color: rgba(var(--glass-rgb), 0.15);
  }

  .about-link:active {
    transform: var(--press-scale);
  }

  .about-link svg {
    color: var(--text-secondary);
    flex-shrink: 0;
  }
</style>
