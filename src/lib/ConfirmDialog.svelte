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
    background: linear-gradient(145deg, #1a1a2e 0%, #16162a 100%);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 16px;
    padding: 24px;
    max-width: 380px;
    width: 100%;
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
    color: #e4e4e7;
  }

  .confirm-message {
    margin: 0 0 20px;
    font-size: 0.875rem;
    color: #9e9ea7;
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

  .btn-confirm {
    padding: 10px 18px;
    font-size: 0.875rem;
    font-weight: 600;
    color: white;
    background: linear-gradient(135deg, #6366f1 0%, #8b5cf6 100%);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s, background-color 0.2s;
  }

  .btn-confirm:hover {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.3);
  }

  .btn-confirm.destructive {
    background: #ef4444;
  }

  .btn-confirm.destructive:hover {
    background: #dc2626;
    box-shadow: 0 4px 12px rgba(239, 68, 68, 0.3);
  }
</style>
