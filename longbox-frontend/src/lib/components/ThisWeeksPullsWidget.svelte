<script lang="ts">
  // Dashboard widget: issues shipping this ship-week for series already
  // on the pull list. Purely informational (no per-row action — these
  // are series you already pull). Renders nothing when there's nothing
  // shipping.
  import type { PullThisWeek } from '$lib/api/releases';

  interface Props {
    rows: PullThisWeek[];
  }

  let { rows }: Props = $props();

  // Capped — a dashboard glance, not the full calendar.
  const MAX = 6;
  const shown = $derived(rows.slice(0, MAX));
</script>

{#if rows.length > 0}
  <section class="mb-6 rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-1 text-base font-semibold">This week's pulls</h2>
    <p class="mb-3 text-xs text-slate-500">
      New issues on sale this week for series on your pull list.
    </p>
    <ul class="space-y-2">
      {#each shown as row (row.cv_issue_id)}
        <li class="flex items-center gap-3">
          {#if row.cover_url}
            <img
              src={row.cover_url}
              alt=""
              class="size-10 flex-shrink-0 rounded bg-slate-100 object-cover"
              loading="lazy"
            />
          {:else}
            <div class="size-10 flex-shrink-0 rounded bg-slate-100" aria-hidden="true"></div>
          {/if}
          <div class="min-w-0 flex-1">
            <a
              href={row.site_detail_url}
              target="_blank"
              rel="noreferrer"
              class="block truncate font-medium text-blue-600 hover:underline"
            >
              {row.volume_name}
            </a>
          </div>
          <span class="whitespace-nowrap font-mono text-sm text-slate-500">
            #{row.issue_number}
          </span>
          <span class="whitespace-nowrap font-mono text-xs text-slate-500">
            {row.store_date}
          </span>
        </li>
      {/each}
    </ul>
    {#if rows.length > MAX}
      <p class="mt-2 text-xs text-slate-400">and {rows.length - MAX} more this week</p>
    {/if}
  </section>
{/if}
