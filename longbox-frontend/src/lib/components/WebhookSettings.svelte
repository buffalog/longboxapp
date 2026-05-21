<script lang="ts">
  // Webhook notification targets — CRUD section of the Settings page.
  // Self-contained: seeded once from the page's load data, then it
  // maintains its own `$state` list from each mutation's response.
  import { Pencil, Send, Trash2 } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    createWebhook,
    deleteWebhook,
    testWebhook,
    updateWebhook,
    WEBHOOK_EVENTS,
    type Webhook,
    type WebhookInput
  } from '$lib/api/webhooks';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from './Button.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  let { webhooks }: { webhooks: Webhook[] } = $props();

  let items = $state<Webhook[]>([...webhooks]);
  let busy = $state(false);
  let error = $state<ApiError | null>(null);

  let showAdd = $state(false);
  let addForm = $state<WebhookInput>(blankForm());
  let editingId = $state<number | null>(null);
  let editForm = $state<WebhookInput>(blankForm());
  let testingId = $state<number | null>(null);

  async function handleTest(id: number): Promise<void> {
    testingId = id;
    try {
      await testWebhook(id);
      toast.success('Test notification delivered.');
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : 'Test delivery failed.');
    } finally {
      testingId = null;
    }
  }

  function blankForm(): WebhookInput {
    return { name: '', url: '', event_mask: 0, enabled: true };
  }

  function toggleBit(form: WebhookInput, bit: number, on: boolean): void {
    form.event_mask = on ? form.event_mask | bit : form.event_mask & ~bit;
  }

  async function run(fn: () => Promise<void>): Promise<void> {
    busy = true;
    error = null;
    try {
      await fn();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      busy = false;
    }
  }

  function handleAdd(): Promise<void> {
    return run(async () => {
      const created = await createWebhook(addForm);
      items = [...items, created];
      addForm = blankForm();
      showAdd = false;
    });
  }

  function startEdit(wh: Webhook): void {
    editingId = wh.id;
    editForm = { name: wh.name, url: wh.url, event_mask: wh.event_mask, enabled: wh.enabled };
  }

  function handleSaveEdit(id: number): Promise<void> {
    return run(async () => {
      const updated = await updateWebhook(id, editForm);
      items = items.map((w) => (w.id === id ? updated : w));
      editingId = null;
    });
  }

  function handleDelete(id: number): Promise<void> {
    return run(async () => {
      await deleteWebhook(id);
      items = items.filter((w) => w.id !== id);
    });
  }

  function eventSummary(mask: number): string {
    const on = WEBHOOK_EVENTS.filter((e) => (mask & e.bit) !== 0).map((e) => e.label);
    return on.length === 0 ? 'no events' : on.join(', ');
  }
</script>

{#snippet fields(form: WebhookInput)}
  <div class="space-y-2">
    <label class="block text-xs font-medium text-slate-600">
      Name
      <input
        type="text"
        bind:value={form.name}
        disabled={busy}
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>
    <label class="block text-xs font-medium text-slate-600">
      URL
      <input
        type="url"
        bind:value={form.url}
        disabled={busy}
        placeholder="https://hooks.slack.com/services/..."
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>
    <fieldset>
      <legend class="mb-1 text-xs font-medium text-slate-600">Events</legend>
      <div class="grid grid-cols-1 gap-1 sm:grid-cols-2">
        {#each WEBHOOK_EVENTS as ev (ev.bit)}
          <label class="flex items-center gap-2 text-sm text-slate-700">
            <input
              type="checkbox"
              checked={(form.event_mask & ev.bit) !== 0}
              disabled={busy}
              onchange={(e) => toggleBit(form, ev.bit, e.currentTarget.checked)}
            />
            {ev.label}
          </label>
        {/each}
      </div>
    </fieldset>
    <label class="flex items-center gap-2 text-sm text-slate-700">
      <input type="checkbox" bind:checked={form.enabled} disabled={busy} />
      Enabled
    </label>
  </div>
{/snippet}

<section class="rounded-lg border border-slate-200 bg-white p-4">
  <header class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
    <h2 class="text-base font-semibold">Webhooks</h2>
    <Button
      variant="ghost"
      size="sm"
      onclick={() => {
        showAdd = !showAdd;
        addForm = blankForm();
      }}
      disabled={busy}
    >
      {showAdd ? 'Cancel' : '+ Add webhook'}
    </Button>
  </header>
  <p class="mb-3 text-sm text-slate-600">
    Notification targets. Slack URLs (<code>hooks.slack.com</code>) get block-kit formatting;
    all others receive plain JSON.
  </p>

  {#if error}
    <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
  {/if}

  {#if showAdd}
    <form
      class="mb-3 rounded-md border border-slate-200 bg-slate-50 p-3"
      onsubmit={(e) => {
        e.preventDefault();
        void handleAdd();
      }}
    >
      {@render fields(addForm)}
      <div class="mt-2">
        <Button
          type="submit"
          disabled={busy || addForm.name.trim() === '' || addForm.url.trim() === ''}
        >
          Add webhook
        </Button>
      </div>
    </form>
  {/if}

  {#if items.length === 0}
    <p class="text-sm text-slate-500">No webhooks configured.</p>
  {:else}
    <ul class="divide-y divide-slate-100 rounded-md border border-slate-200">
      {#each items as wh (wh.id)}
        <li class="px-3 py-2 text-sm">
          {#if editingId === wh.id}
            <form
              onsubmit={(e) => {
                e.preventDefault();
                void handleSaveEdit(wh.id);
              }}
            >
              {@render fields(editForm)}
              <div class="mt-2 flex gap-2">
                <Button
                  type="submit"
                  disabled={busy || editForm.name.trim() === '' || editForm.url.trim() === ''}
                >
                  Save
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onclick={() => (editingId = null)}
                  disabled={busy}
                >
                  Cancel
                </Button>
              </div>
            </form>
          {:else}
            <div class="flex items-center justify-between gap-3">
              <div class="min-w-0">
                <span class="font-medium">{wh.name}</span>
                {#if !wh.enabled}
                  <span class="ml-1 text-xs text-slate-400">(disabled)</span>
                {/if}
                <div class="truncate text-xs text-slate-500">{wh.url}</div>
                <div class="text-xs text-slate-400">{eventSummary(wh.event_mask)}</div>
              </div>
              <div class="flex flex-shrink-0 items-center gap-1">
                <button
                  type="button"
                  class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:opacity-50"
                  aria-label={`Test webhook ${wh.name}`}
                  onclick={() => handleTest(wh.id)}
                  disabled={busy || testingId !== null}
                >
                  <Send class="size-4" aria-hidden="true" />
                </button>
                <button
                  type="button"
                  class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:opacity-50"
                  aria-label={`Edit webhook ${wh.name}`}
                  onclick={() => startEdit(wh)}
                  disabled={busy}
                >
                  <Pencil class="size-4" aria-hidden="true" />
                </button>
                <button
                  type="button"
                  class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-red-700 focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:opacity-50"
                  aria-label={`Remove webhook ${wh.name}`}
                  onclick={() => handleDelete(wh.id)}
                  disabled={busy}
                >
                  <Trash2 class="size-4" aria-hidden="true" />
                </button>
              </div>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>
