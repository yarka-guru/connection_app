<script>
import { trapFocus } from './utils.js'

// onApply(choices) returns a promise; a rejection is shown here and the dialog stays open.
// onDetectAccount(profile) asks AWS for the profile's account and resolves to a fresh proposal.
const { proposals = [], onApply, onLater, onDetectAccount } = $props()

// Editable copy of a proposal. Rows the user already saw keep their edits
// (matched by project and account).
function editable(list, previous = []) {
  return list.map((p) => {
    const before = previous.find((q) => q.projectKey === p.projectKey)
    return {
      projectKey: p.projectKey,
      projectName: p.projectName,
      skipped: p.skipped || [],
      environments: p.environments.map((e) => {
        const old = before?.environments.find((o) => o.accountId === e.accountId)
        return old ? { ...e, name: old.name, port: old.port, selected: old.selected } : { ...e }
      }),
    }
  })
}

let projects = $state(editable(proposals))
let applying = $state(false)
let error = $state('')
let detecting = $state(null)
let detectErrors = $state({})

async function detect(profile) {
  if (detecting || !onDetectAccount) return
  detecting = profile
  detectErrors = Object.fromEntries(Object.entries(detectErrors).filter(([name]) => name !== profile))
  try {
    const fresh = await onDetectAccount(profile)
    projects = editable(fresh, projects)
  } catch (err) {
    detectErrors = { ...detectErrors, [profile]: `${err}` }
  } finally {
    detecting = null
  }
}

const selectedCount = $derived(
  projects.reduce((n, p) => n + p.environments.filter((e) => e.selected).length, 0),
)

async function apply() {
  if (applying) return
  applying = true
  error = ''
  try {
    await onApply(
      projects.map((p) => ({
        projectKey: p.projectKey,
        environments: p.environments.map((e) => ({ ...e })),
      })),
    )
  } catch (err) {
    error = `${err}`
  } finally {
    applying = false
  }
}

function handleKeydown(e) {
  if (e.key === 'Escape' && !applying) onLater?.()
}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="dialog-overlay" onclick={() => { if (!applying) onLater?.() }} onkeydown={handleKeydown}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="dialog" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key !== 'Escape') e.stopPropagation() }} use:trapFocus role="dialog" tabindex="-1" aria-label="Update projects to environments">
    <h3>Update projects to environments</h3>
    <p class="intro">
      Projects now use AWS accounts instead of profile names. Review the environments
      found for each project. A backup of projects.json is made before anything changes.
    </p>

    <div class="scroll">
      {#each projects as project (project.projectKey)}
        <section class="project">
          <h4>{project.projectName || project.projectKey}</h4>
          {#if project.environments.length > 0}
            <table>
              <thead>
                <tr>
                  <th class="col-use"><span class="sr-only">Include</span></th>
                  <th>Name</th>
                  <th>Port</th>
                  <th>Account</th>
                </tr>
              </thead>
              <tbody>
                {#each project.environments as env, ei (ei)}
                  <tr class:off={!env.selected}>
                    <td class="col-use">
                      <input
                        type="checkbox"
                        bind:checked={env.selected}
                        aria-label={`Include ${env.name || 'environment'} in ${project.projectName || project.projectKey}`}
                      />
                    </td>
                    <td><input type="text" bind:value={env.name} aria-label="Environment name" disabled={!env.selected} /></td>
                    <td><input class="port" type="text" inputmode="numeric" bind:value={env.port} aria-label="Local port" disabled={!env.selected} /></td>
                    <td class="account" title={env.profiles.join(', ')}>{env.accountId}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {:else}
            <p class="muted">No environments could be proposed.</p>
          {/if}
          {#if project.skipped.length > 0}
            <div class="skipped">
              <span class="skipped-title">Skipped</span>
              <ul>
                {#each project.skipped as s (s.profile)}
                  <li>
                    <code>{s.profile}</code>: {s.reason}
                    {#if s.accountUnknown && onDetectAccount}
                      <button
                        type="button"
                        class="btn-detect"
                        onclick={() => detect(s.profile)}
                        disabled={detecting !== null || applying}
                      >
                        {detecting === s.profile ? 'Detecting…' : 'Detect account'}
                      </button>
                    {/if}
                    {#if detectErrors[s.profile]}
                      <span class="detect-error" role="alert">{detectErrors[s.profile]}</span>
                    {/if}
                  </li>
                {/each}
              </ul>
            </div>
          {/if}
        </section>
      {/each}
    </div>

    {#if error}
      <div class="error" role="alert">{error}</div>
    {/if}

    <div class="actions">
      <button type="button" class="btn-cancel" onclick={onLater} disabled={applying}>Later</button>
      <button type="button" class="btn-confirm" onclick={apply} disabled={applying || selectedCount === 0}>
        {applying ? 'Updating...' : 'Apply'}
      </button>
    </div>
  </div>
</div>

<style>
  .dialog-overlay {
    position: fixed;
    inset: 0;
    background: var(--overlay-bg);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
    padding: 24px;
  }

  .dialog {
    background: var(--bg-card-inner);
    -webkit-backdrop-filter: var(--glass-blur-heavy);
    backdrop-filter: var(--glass-blur-heavy);
    border: 1px solid var(--glass-border);
    border-radius: 16px;
    padding: 24px;
    width: 600px;
    max-width: 100%;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    box-shadow: var(--glass-inner-glow), var(--glass-shadow);
  }

  h3 {
    margin: 0 0 8px;
    font-size: 1.1rem;
    color: var(--text-primary);
  }

  h4 {
    margin: 0 0 8px;
    font-size: 0.9rem;
    color: var(--text-primary);
  }

  .intro {
    margin: 0 0 16px;
    font-size: 0.85rem;
    line-height: 1.5;
    color: var(--text-secondary);
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding-right: 4px;
  }

  .project {
    margin-bottom: 20px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  th {
    text-align: left;
    font-size: 0.75rem;
    font-weight: 500;
    color: #9e9ea7;
    padding: 0 6px 6px;
  }

  td {
    padding: 4px 6px;
    vertical-align: middle;
  }

  .col-use {
    width: 28px;
  }

  tr.off td:not(.col-use) {
    opacity: 0.55;
  }

  td input[type='text'] {
    width: 100%;
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 8px;
    padding: 7px 10px;
    font-size: 0.85rem;
    color: var(--text-primary);
    outline: none;
  }

  td input[type='text']:focus {
    border-color: var(--accent-primary);
  }

  td input.port {
    width: 80px;
  }

  .account {
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.8rem;
    color: #9e9ea7;
  }

  .muted {
    margin: 0;
    font-size: 0.85rem;
    color: #9e9ea7;
  }

  .skipped {
    margin-top: 8px;
    font-size: 0.8rem;
    color: #9e9ea7;
  }

  .skipped-title {
    font-weight: 600;
  }

  .skipped ul {
    margin: 4px 0 0;
    padding-left: 18px;
  }

  .skipped li {
    margin-bottom: 4px;
  }

  .btn-detect {
    margin-left: 6px;
    padding: 2px 8px;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--accent-primary-light);
    background: rgba(var(--accent-primary-rgb), 0.1);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.2);
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-detect:hover:not(:disabled) {
    background: rgba(var(--accent-primary-rgb), 0.15);
  }

  .btn-detect:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .detect-error {
    display: block;
    margin-top: 2px;
    color: var(--color-error-soft);
    overflow-wrap: anywhere;
  }

  .error {
    margin: 12px 0 0;
    padding: 10px 12px;
    font-size: 0.8rem;
    line-height: 1.4;
    color: var(--color-error);
    background: rgba(var(--color-error-rgb), 0.1);
    border: 1px solid rgba(var(--color-error-rgb), 0.25);
    border-radius: 8px;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
    margin-top: 16px;
  }

  .btn-cancel {
    padding: 10px 18px;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-cancel:hover:not(:disabled) {
    background: rgba(var(--glass-rgb), 0.05);
    color: var(--text-hover);
  }

  .btn-confirm {
    padding: 10px 18px;
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--button-text);
    background: var(--bg-button-gradient);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s;
  }

  .btn-confirm:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px var(--bg-button-gradient-shadow);
  }

  .btn-cancel:disabled,
  .btn-confirm:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
