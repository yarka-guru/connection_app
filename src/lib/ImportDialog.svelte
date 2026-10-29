<script>
import { trapFocus } from './utils.js'

// onApply(choices) returns a promise; a rejection is shown here and the dialog stays open.
const { preview, onApply, onCancel } = $props()

const CHOICES = [
  { value: 'replace', label: 'Replace' },
  { value: 'keepMine', label: 'Keep mine' },
  { value: 'copy', label: 'Import as copy' },
]

// Item key -> chosen action, starting from the backend's suggestion
const choices = $state(
  Object.fromEntries(preview.items.map((item) => [item.key, item.defaultChoice])),
)
let applying = $state(false)
let error = $state('')

const importable = $derived(preview.items.filter((i) => i.status !== 'broken'))

async function apply() {
  if (applying) return
  applying = true
  error = ''
  try {
    const picked = {}
    for (const item of importable) picked[item.key] = choices[item.key]
    await onApply(picked)
  } catch (err) {
    error = `${err}`
  } finally {
    applying = false
  }
}

function handleKeydown(e) {
  if (e.key === 'Escape' && !applying) onCancel?.()
}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="dialog-overlay" onclick={() => { if (!applying) onCancel?.() }} onkeydown={handleKeydown}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="dialog" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key !== 'Escape') e.stopPropagation() }} use:trapFocus role="dialog" tabindex="-1" aria-label="Import projects">
    <h3>Import projects</h3>
    <p class="path" title={preview.path}>{preview.path}</p>

    {#if preview.items.length === 0}
      <p class="muted">The file contains no projects.</p>
    {/if}

    <div class="scroll">
      {#each preview.items as item (item.key)}
        <div class="item" class:broken={item.status === 'broken'}>
          <div class="item-head">
            <span class="item-key">{item.key}</span>
            {#if item.status === 'new'}
              <span class="badge badge-new">New</span>
            {:else if item.status === 'conflict'}
              <span class="badge badge-conflict">Already exists</span>
            {:else}
              <span class="badge badge-broken">Broken</span>
            {/if}
          </div>

          {#if item.status === 'conflict'}
            {#if item.localMigrated && item.importedLegacy}
              <p class="note">The file uses the old format; your project has already been updated.</p>
            {/if}
            <div class="radios" role="radiogroup" aria-label={`What to do with ${item.key}`}>
              {#each CHOICES as c (c.value)}
                <label>
                  <input type="radio" name={`choice-${item.key}`} value={c.value} bind:group={choices[item.key]} />
                  {c.label}
                </label>
              {/each}
            </div>
          {:else if item.status === 'new'}
            <p class="note">Will be imported.</p>
          {:else}
            <p class="problem">{item.error || 'This project cannot be read.'}</p>
            <p class="note">Skipped.</p>
          {/if}
        </div>
      {/each}
    </div>

    {#if error}
      <div class="error" role="alert">{error}</div>
    {/if}

    <div class="actions">
      <button type="button" class="btn-cancel" onclick={onCancel} disabled={applying}>Cancel</button>
      <button type="button" class="btn-confirm" onclick={apply} disabled={applying || importable.length === 0}>
        {applying ? 'Importing...' : 'Import'}
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
    width: 520px;
    max-width: 100%;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    box-shadow: var(--glass-inner-glow), var(--glass-shadow);
  }

  h3 {
    margin: 0 0 6px;
    font-size: 1.1rem;
    color: var(--text-primary);
  }

  .path {
    margin: 0 0 16px;
    font-size: 0.75rem;
    color: #8b8b95;
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding-right: 4px;
  }

  .item {
    padding: 10px 12px;
    margin-bottom: 8px;
    border: 1px solid rgba(var(--glass-rgb), 0.08);
    border-radius: 10px;
  }

  .item.broken {
    border-color: rgba(var(--color-error-rgb), 0.3);
  }

  .item-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .item-key {
    font-size: 0.9rem;
    font-weight: 500;
    color: var(--text-primary);
  }

  .badge {
    font-size: 0.7rem;
    padding: 2px 8px;
    border-radius: 10px;
    background: rgba(var(--glass-rgb), 0.08);
    color: var(--text-secondary);
  }

  .badge-broken {
    color: var(--color-error);
    background: rgba(var(--color-error-rgb), 0.1);
  }

  .note,
  .muted {
    margin: 6px 0 0;
    font-size: 0.8rem;
    color: #9e9ea7;
  }

  .problem {
    margin: 6px 0 0;
    font-size: 0.8rem;
    color: var(--color-error);
    overflow-wrap: anywhere;
  }

  .radios {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    margin-top: 8px;
  }

  .radios label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.85rem;
    color: var(--text-hover);
    cursor: pointer;
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
</style>
