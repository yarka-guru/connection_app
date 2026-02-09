<script>
import { onDestroy } from 'svelte'
import { copyToClipboard, safeTimeout } from './utils.js'

const {
  value = '',
  label = 'Copy',
  size = 14,
  onError,
} = $props()

let copied = $state(false)
let cancelTimeout = null

async function handleCopy() {
  const ok = await copyToClipboard(value)
  if (ok) {
    copied = true
    cancelTimeout?.()
    cancelTimeout = safeTimeout(() => {
      copied = false
    }, 2000)
  } else {
    onError?.('Failed to copy to clipboard')
  }
}

onDestroy(() => {
  cancelTimeout?.()
})
</script>

<button
  class="copy-btn"
  class:copied
  onclick={handleCopy}
  aria-label={copied ? 'Copied' : label}
>
  {#if copied}
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none">
      <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
    </svg>
  {:else}
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none">
      <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
      <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
    </svg>
  {/if}
</button>

<style>
  .copy-btn {
    width: 26px;
    height: 26px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: #71717a;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
    flex-shrink: 0;
  }

  .copy-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    color: #a1a1aa;
  }

  .copy-btn.copied {
    color: #34d399;
  }
</style>
