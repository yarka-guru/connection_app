<script>
import { trapFocus } from './utils.js'

const {
  title = 'Confirm',
  message = 'Are you sure?',
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  destructive = false,
  onConfirm,
  onCancel,
} = $props()

function handleKeydown(e) {
  if (e.key === 'Escape') {
    onCancel?.()
  }
}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="confirm-overlay" onclick={onCancel} onkeydown={handleKeydown}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="confirm-dialog" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()} use:trapFocus role="alertdialog" tabindex="-1" aria-label={title}>
    <h3 class="confirm-title">{title}</h3>
    <p class="confirm-message">{message}</p>
    <div class="confirm-actions">
      <button class="btn-cancel" onclick={onCancel}>{cancelLabel}</button>
      <button
        class="btn-confirm"
        class:destructive
        onclick={onConfirm}
      >
        {confirmLabel}
      </button>
    </div>
  </div>
</div>

<style>
  .confirm-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
    padding: 24px;
    animation: fadeIn 0.15s ease-out;
  }

  @keyframes fadeIn {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  .confirm-dialog {
    background: rgba(26, 43, 31, 0.85);
    -webkit-backdrop-filter: var(--glass-blur-heavy);
    backdrop-filter: var(--glass-blur-heavy);
    border: 1px solid var(--glass-border);
    border-radius: 16px;
    padding: 24px;
    max-width: 380px;
    width: 100%;
    box-shadow: var(--glass-inner-glow), var(--glass-shadow);
    animation: slideUp 0.2s ease-out;
  }

  @keyframes slideUp {
    from { opacity: 0; transform: translateY(10px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .confirm-title {
    margin: 0 0 8px;
    font-size: 1.1rem;
    font-weight: 600;
    color: #d5ddd3;
  }

  .confirm-message {
    margin: 0 0 20px;
    font-size: 0.875rem;
    color: #8a9488;
    line-height: 1.5;
  }

  .confirm-actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
  }

  .btn-cancel {
    padding: 10px 18px;
    font-size: 0.875rem;
    font-weight: 500;
    color: #6b7d6a;
    background: none;
    border: 1px solid rgba(200, 220, 195, 0.1);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-cancel:hover {
    background: rgba(200, 220, 195, 0.05);
    color: #9baa98;
  }

  .btn-cancel:active {
    transform: var(--press-scale);
  }

  .btn-confirm {
    padding: 10px 18px;
    font-size: 0.875rem;
    font-weight: 600;
    color: white;
    background: linear-gradient(135deg, #d4a853 0%, #7aab6d 100%);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s, background-color 0.2s;
  }

  .btn-confirm:hover {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(212, 168, 83, 0.3);
  }

  .btn-confirm:active {
    transform: var(--press-scale);
  }

  .btn-confirm.destructive {
    background: #c9614a;
  }

  .btn-confirm.destructive:hover {
    background: #b0503c;
    box-shadow: 0 4px 12px rgba(201, 97, 74, 0.3);
  }
</style>
