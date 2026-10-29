<script>
// Editor for a project's account-based environments: one row per
// environment (name, AWS account, local port) with optional overrides.
const ACCOUNT_ID = /^\d{12}$/

const RDS_OVERRIDES = [
  { key: 'rdsPattern', label: 'RDS pattern', placeholder: "project's RDS pattern" },
  { key: 'region', label: 'Region', placeholder: "project's region" },
]
const SERVICE_OVERRIDES = [
  { key: 'targetPattern', label: 'EC2 name pattern', placeholder: "project's pattern" },
  { key: 'ecsCluster', label: 'ECS cluster', placeholder: "project's cluster" },
  { key: 'ecsService', label: 'ECS service', placeholder: "project's service" },
  { key: 'bastionPattern', label: 'Bastion pattern', placeholder: "project's pattern" },
]

let {
  environments = $bindable([]),
  // ProfileAccount[] from list_aws_accounts: { profile, accountId }
  accounts = [],
  // Other projects to copy environments from: { key, name, environments }
  projects = [],
  connectionType = 'rds',
  defaultPort = '',
  // FieldError[] whose field starts with "environments["
  errors = [],
  // accountId -> the profile the app uses when an account has several
  chosenProfiles = {},
  onCreateProfile,
  onPickProfile,
} = $props()

const uid = $props.id()
const datalistId = `${uid}-accounts`

let expanded = $state([])

const overrideFields = $derived(connectionType === 'service' ? SERVICE_OVERRIDES : RDS_OVERRIDES)

// One datalist option per known account, labelled with its profiles
const accountOptions = $derived.by(() => {
  const byAccount = new Map()
  for (const a of accounts) {
    if (!a.accountId) continue
    const profiles = byAccount.get(a.accountId) ?? []
    profiles.push(a.profile)
    byAccount.set(a.accountId, profiles)
  }
  return [...byAccount].map(([id, profiles]) => ({ id, label: profiles.join(', ') }))
})

function errorFor(index, field) {
  const name = `environments[${index}].${field}`
  return errors
    .filter((e) => e.field === name)
    .map((e) => e.message)
    .join('; ')
}

function overrideErrorCount(index) {
  return overrideFields.filter((f) => errorFor(index, f.key)).length
}

function overrideCount(env) {
  return overrideFields.filter((f) => env[f.key]?.trim()).length
}

function candidatesFor(accountId) {
  return accounts.filter((a) => a.accountId === accountId).map((a) => a.profile)
}

// Accept a profile name or an ID typed with spaces/dashes, store the bare ID
function normalizeAccount(env) {
  const value = (env.accountId ?? '').trim()
  const byProfile = accounts.find((a) => a.profile === value && a.accountId)
  if (byProfile) {
    env.accountId = byProfile.accountId
    return
  }
  const digits = value.replace(/[\s-]/g, '')
  env.accountId = ACCOUNT_ID.test(digits) ? digits : value
}

function nextPort() {
  const ports = environments.map((e) => Number.parseInt(e.port, 10)).filter(Number.isFinite)
  return ports.length > 0 ? String(Math.max(...ports) + 1) : String(defaultPort ?? '')
}

function addEnvironment() {
  environments = [...environments, { name: '', accountId: '', port: nextPort() }]
  expanded = [...expanded, false]
}

function removeEnvironment(index) {
  environments = environments.filter((_, i) => i !== index)
  expanded = expanded.filter((_, i) => i !== index)
}

function toggleOverrides(index) {
  const next = [...expanded]
  next[index] = !next[index]
  expanded = next
}

function copyFrom(event) {
  const source = projects.find((p) => p.key === event.target.value)
  event.target.value = ''
  if (!source) return
  environments = (source.environments ?? []).map((e) => ({
    name: e.name,
    accountId: e.accountId,
    port: String(e.port),
  }))
  expanded = environments.map(() => false)
}
</script>

<div class="environments">
  <div class="env-toolbar">
    <button type="button" class="btn-add-small" onclick={addEnvironment}>+ Add environment</button>
    {#if projects.length > 0}
      <select class="copy-select" aria-label="Copy environments from another project" onchange={copyFrom}>
        <option value="">Copy from…</option>
        {#each projects as p (p.key)}
          <option value={p.key}>{p.name} ({p.environments.length})</option>
        {/each}
      </select>
    {/if}
  </div>

  {#if environments.length === 0}
    <p class="env-empty">No environments yet. Add one per AWS account this project connects to.</p>
  {:else}
    <div class="env-head" aria-hidden="true">
      <span>Name</span>
      <span>AWS account</span>
      <span>Local port</span>
    </div>
  {/if}

  {#each environments as env, i}
    {@const nameError = errorFor(i, 'name')}
    {@const accountError = errorFor(i, 'accountId')}
    {@const portError = errorFor(i, 'port')}
    {@const overridesOpen = expanded[i] || overrideErrorCount(i) > 0}
    {@const candidates = ACCOUNT_ID.test(env.accountId ?? '') ? candidatesFor(env.accountId) : null}
    <div class="env-row" class:has-error={nameError || accountError || portError}>
      <div class="env-fields">
        <input
          type="text"
          bind:value={env.name}
          placeholder="prod"
          aria-label="Environment {i + 1} name"
          aria-invalid={nameError ? 'true' : undefined}
        />
        <input
          type="text"
          list={datalistId}
          bind:value={env.accountId}
          onchange={() => normalizeAccount(env)}
          placeholder="Account ID or profile"
          aria-label="Environment {i + 1} AWS account"
          aria-invalid={accountError ? 'true' : undefined}
        />
        <input
          type="text"
          inputmode="numeric"
          bind:value={env.port}
          placeholder={defaultPort || 'port'}
          aria-label="Environment {i + 1} local port"
          aria-invalid={portError ? 'true' : undefined}
        />
        <button
          type="button"
          class="btn-overrides"
          class:active={overridesOpen}
          onclick={() => toggleOverrides(i)}
          aria-expanded={overridesOpen}
          aria-label="Overrides for environment {i + 1}"
          title="Overrides"
        >
          <svg width="12" height="12" viewBox="0 0 14 14" fill="none">
            <path d="M5 3l4 4-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
          {#if overrideCount(env) > 0}<span class="override-count">{overrideCount(env)}</span>{/if}
        </button>
        <button type="button" class="btn-remove" onclick={() => removeEnvironment(i)} aria-label="Remove environment {i + 1}">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
          </svg>
        </button>
      </div>

      {#if nameError}<span class="field-error">{nameError}</span>{/if}
      {#if accountError}<span class="field-error">{accountError}</span>{/if}
      {#if portError}<span class="field-error">{portError}</span>{/if}

      {#if candidates}
        <div class="env-status">
          {#if candidates.length === 0}
            <span class="status-missing">No AWS profile for this account</span>
            {#if onCreateProfile}
              <button type="button" class="btn-link" onclick={() => onCreateProfile(env.accountId)}>Create profile</button>
            {/if}
          {:else if candidates.length === 1}
            <span class="status-text">via {candidates[0]}</span>
          {:else}
            <label class="status-text" for="{uid}-pick-{i}">pick profile</label>
            <select
              id="{uid}-pick-{i}"
              class="pick-select"
              value={chosenProfiles[env.accountId] ?? candidates[0]}
              onchange={(e) => onPickProfile?.(env.accountId, e.target.value)}
            >
              {#each candidates as profile (profile)}
                <option value={profile}>{profile}</option>
              {/each}
            </select>
          {/if}
        </div>
      {/if}

      {#if overridesOpen}
        <div class="env-overrides">
          <span class="overrides-hint">Leave empty to use the project's value.</span>
          {#each overrideFields as field (field.key)}
            {@const overrideError = errorFor(i, field.key)}
            <div class="override-field">
              <label for="{uid}-{i}-{field.key}">{field.label}</label>
              <input
                id="{uid}-{i}-{field.key}"
                type="text"
                bind:value={env[field.key]}
                placeholder={field.placeholder}
                aria-invalid={overrideError ? 'true' : undefined}
              />
              {#if overrideError}<span class="field-error">{overrideError}</span>{/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/each}

  <datalist id={datalistId}>
    {#each accountOptions as option (option.id)}
      <option value={option.id}>{option.label}</option>
    {/each}
  </datalist>
</div>

<style>
  .environments {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .env-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 4px;
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

  .copy-select,
  .pick-select {
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 0.75rem;
    color: var(--text-primary);
    outline: none;
    cursor: pointer;
  }

  .copy-select:focus,
  .pick-select:focus {
    border-color: var(--accent-primary);
  }

  .env-empty {
    margin: 4px 0;
    font-size: 0.75rem;
    color: #9e9ea7;
  }

  .env-head,
  .env-fields {
    display: grid;
    grid-template-columns: 1fr 1.5fr 0.8fr 28px 28px;
    gap: 6px;
    align-items: center;
  }

  .env-head span {
    font-size: 0.7rem;
    font-weight: 500;
    color: #9e9ea7;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .env-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-bottom: 6px;
    border-bottom: 1px solid rgba(var(--glass-rgb), 0.06);
  }

  .env-row input {
    width: 100%;
    min-width: 0;
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 6px;
    padding: 8px 10px;
    font-size: 0.8rem;
    color: var(--text-primary);
    outline: none;
  }

  .env-row input:focus {
    border-color: var(--accent-primary);
  }

  .env-row input[aria-invalid='true'] {
    border-color: rgba(var(--color-error-rgb), 0.6);
  }

  .btn-overrides,
  .btn-remove {
    position: relative;
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    background: none;
    border: none;
    color: var(--text-muted);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-overrides svg {
    transition: transform 0.2s;
  }

  .btn-overrides.active svg {
    transform: rotate(90deg);
  }

  .btn-overrides:hover {
    background: rgba(var(--glass-rgb), 0.1);
    color: var(--text-hover);
  }

  .btn-remove:hover {
    background: rgba(var(--color-error-rgb), 0.1);
    color: var(--color-error-soft);
  }

  .override-count {
    position: absolute;
    top: 0;
    right: 0;
    font-size: 0.6rem;
    line-height: 1;
    padding: 1px 3px;
    border-radius: 4px;
    color: var(--accent-primary);
    background: rgba(var(--accent-primary-rgb), 0.15);
  }

  .env-status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.72rem;
  }

  .status-text {
    color: #9e9ea7;
  }

  .status-missing {
    color: var(--color-warning, #e0a63a);
  }

  .btn-link {
    padding: 0;
    background: none;
    border: none;
    font-size: 0.72rem;
    font-weight: 500;
    color: var(--accent-primary-light);
    cursor: pointer;
    text-decoration: underline;
    transition: color 0.2s;
  }

  .btn-link:hover {
    color: var(--text-hover);
  }

  .env-overrides {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    padding: 8px;
    border-radius: 8px;
    background: rgba(var(--glass-rgb), 0.03);
  }

  .overrides-hint {
    grid-column: 1 / -1;
    font-size: 0.7rem;
    color: #9e9ea7;
  }

  .override-field label {
    display: block;
    font-size: 0.7rem;
    font-weight: 500;
    color: var(--text-hover);
    margin-bottom: 4px;
  }

  .field-error {
    display: block;
    font-size: 0.7rem;
    color: var(--color-error-soft);
  }
</style>
