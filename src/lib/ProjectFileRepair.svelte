<script>
// Banner shown when projects.json as a whole cannot be read: the parser's
// position, a raw editor for the whole file, and restoring a backup.
import { tick } from 'svelte'
import ConfirmDialog from './ConfirmDialog.svelte'

const { fileError, backups = [], invoke, onRepaired } = $props()

let editorOpen = $state(false)
let text = $state('')
let busy = $state(false)
let message = $state('')
let chosenBackup = $state('')
let confirmRestore = $state(false)
let textarea = $state(null)

const selectedBackup = $derived(backups.includes(chosenBackup) ? chosenBackup : (backups[0] ?? ''))

const description = $derived.by(() => {
  if (fileError.kind === 'syntax') {
    return `Syntax error at line ${fileError.line}, column ${fileError.column}: ${fileError.message}`
  }
  if (fileError.kind === 'notAnObject') return 'The file must contain a JSON object.'
  return fileError.message
})

function backupLabel(path) {
  const name = path.split(/[\\/]/).pop()
  const m = name.match(/bak-(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})$/)
  return m ? `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}:${m[6]}` : name
}

// Character offset of a 1-based line and column
function offsetOf(source, line, column) {
  let offset = 0
  for (let l = 1; l < line; l++) {
    const next = source.indexOf('\n', offset)
    if (next === -1) return source.length
    offset = next + 1
  }
  return Math.min(offset + Math.max(column - 1, 0), source.length)
}

async function openEditor() {
  busy = true
  message = ''
  try {
    text = await invoke('get_project_file_raw')
    editorOpen = true
    await tick()
    if (fileError.kind === 'syntax' && textarea) {
      const at = offsetOf(text, fileError.line, fileError.column)
      textarea.focus()
      textarea.setSelectionRange(at, at)
    }
  } catch (err) {
    message = `Failed to read projects.json: ${err}`
  } finally {
    busy = false
  }
}

async function saveFile() {
  busy = true
  message = ''
  try {
    await invoke('save_project_file_raw', { text })
    editorOpen = false
    await onRepaired?.('projects.json saved')
  } catch (err) {
    message = `Not saved: ${err}`
  } finally {
    busy = false
  }
}

async function restore() {
  confirmRestore = false
  busy = true
  message = ''
  try {
    await invoke('restore_project_backup', { path: selectedBackup })
    editorOpen = false
    await onRepaired?.('Backup restored')
  } catch (err) {
    message = `Failed to restore backup: ${err}`
  } finally {
    busy = false
  }
}
</script>

<div class="file-error" role="alert">
  <div class="file-error-title">projects.json can't be read</div>
  <p class="file-error-detail">{description}</p>
  <p class="file-error-hint">No projects are loaded until the file is fixed. The file is only changed when you save it or restore a backup; the current file is backed up first.</p>

  <div class="file-error-actions">
    {#if !editorOpen}
      <button type="button" class="btn-repair" onclick={openEditor} disabled={busy}>Edit file</button>
    {/if}
    {#if backups.length > 0}
      <select
        class="backup-select"
        aria-label="Backup to restore"
        value={selectedBackup}
        onchange={(e) => { chosenBackup = e.target.value }}
      >
        {#each backups as path (path)}
          <option value={path}>{backupLabel(path)}</option>
        {/each}
      </select>
      <button type="button" class="btn-repair" onclick={() => { confirmRestore = true }} disabled={busy || !selectedBackup}>
        Restore backup
      </button>
    {/if}
  </div>

  {#if message}
    <p class="file-error-message">{message}</p>
  {/if}

  {#if editorOpen}
    <textarea
      class="file-editor"
      bind:this={textarea}
      bind:value={text}
      spellcheck="false"
      aria-label="projects.json contents"
    ></textarea>
    <div class="file-editor-actions">
      <button type="button" class="btn-repair secondary" onclick={() => { editorOpen = false }} disabled={busy}>Cancel</button>
      <button type="button" class="btn-repair" onclick={saveFile} disabled={busy}>
        {busy ? 'Saving...' : 'Save file'}
      </button>
    </div>
  {/if}
</div>

{#if confirmRestore}
  <ConfirmDialog
    title="Restore backup"
    message="Replace projects.json with the backup from {backupLabel(selectedBackup)}? The current file is backed up first."
    confirmLabel="Restore"
    onConfirm={restore}
    onCancel={() => { confirmRestore = false }}
  />
{/if}

<style>
  .file-error {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px;
    margin-bottom: 16px;
    border-radius: 12px;
    background: rgba(var(--color-error-rgb), 0.08);
    border: 1px solid rgba(var(--color-error-rgb), 0.25);
  }

  .file-error-title {
    font-weight: 600;
    font-size: 0.9rem;
    color: var(--color-error-soft);
  }

  .file-error-detail {
    margin: 0;
    font-size: 0.8rem;
    color: var(--text-primary);
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    word-break: break-word;
  }

  .file-error-hint {
    margin: 0;
    font-size: 0.75rem;
    color: #9e9ea7;
    line-height: 1.4;
  }

  .file-error-message {
    margin: 0;
    font-size: 0.8rem;
    color: var(--color-error-soft);
  }

  .file-error-actions,
  .file-editor-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .file-editor-actions {
    justify-content: flex-end;
  }

  .btn-repair {
    padding: 6px 12px;
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--accent-primary-light);
    background: rgba(var(--accent-primary-rgb), 0.1);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.2);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-repair:hover:not(:disabled) {
    background: rgba(var(--accent-primary-rgb), 0.15);
  }

  .btn-repair:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-repair.secondary {
    color: var(--text-muted);
    background: none;
    border-color: rgba(var(--glass-rgb), 0.1);
  }

  .backup-select {
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 8px;
    padding: 6px 8px;
    font-size: 0.8rem;
    color: var(--text-primary);
    outline: none;
    cursor: pointer;
  }

  .backup-select:focus {
    border-color: var(--accent-primary);
  }

  .file-editor {
    min-height: 240px;
    background: var(--input-bg);
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 10px;
    padding: 12px;
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.78rem;
    color: var(--text-primary);
    resize: vertical;
    outline: none;
  }

  .file-editor:focus {
    border-color: var(--accent-primary);
  }
</style>
