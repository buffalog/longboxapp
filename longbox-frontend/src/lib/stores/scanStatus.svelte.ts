// Cross-route scan-status store. Polls /api/scans/current and
// /api/scans/recent at an adaptive interval (2s while scanning, 30s idle).
// Components subscribe in onMount; the store stops polling when no
// subscribers remain.

import { ApiError } from '../api/client';
import * as scansApi from '../api/scans';
import { toast } from './toast.svelte';
import type { CurrentScan, ScanRun } from '../types';

const ACTIVE_INTERVAL_MS = 2000;
const IDLE_INTERVAL_MS = 30000;

class ScanStatusStore {
  current = $state<CurrentScan | null>(null);
  recent = $state<ScanRun[]>([]);
  error = $state<ApiError | null>(null);

  private timer: ReturnType<typeof setTimeout> | null = null;
  private subscribers = 0;
  /** Watches the running→idle transition so we can fire a one-shot
   *  scan-completion toast. The store sees the transition in `tick()`
   *  when `current` flips from non-null to null. */
  private previousCurrent: CurrentScan | null = null;
  /** ID of the most recently toasted scan run, so a tick that catches
   *  the same completed row again (e.g. after a manual refresh) doesn't
   *  re-toast. Initialized lazily on the first tick — the very first
   *  poll's `recent[0]` is established history, not something to toast
   *  on. */
  private lastToastedScanId: number | null = null;
  private initialized = false;

  subscribe(): () => void {
    this.subscribers += 1;
    if (this.subscribers === 1) {
      void this.start();
    }
    return () => {
      this.subscribers -= 1;
      if (this.subscribers === 0) {
        this.stop();
      }
    };
  }

  /** Trigger an immediate poll. Useful after a mutation. */
  async refresh(): Promise<void> {
    await this.tick();
  }

  private async start(): Promise<void> {
    await this.tick();
  }

  private stop(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  private async tick(): Promise<void> {
    try {
      const [current, recent] = await Promise.all([
        scansApi.getCurrent(),
        scansApi.getRecent()
      ]);
      const wasScanning = this.previousCurrent !== null;
      this.current = current;
      this.recent = recent;
      this.error = null;

      // Scan-completion toast (ITEM 11). Fire on the running→idle
      // transition when the newest recent row is fresh AND we haven't
      // already toasted it. The `initialized` gate suppresses a spurious
      // toast for the historical top-of-recent on first load.
      const latest = recent[0];
      if (this.initialized && wasScanning && current === null && latest) {
        if (latest.id !== this.lastToastedScanId) {
          this.lastToastedScanId = latest.id;
          this.fireScanCompletionToast(latest);
        }
      } else if (!this.initialized) {
        // Seed `lastToastedScanId` with whatever was at the top of
        // recent on first poll so we don't toast for a scan that
        // completed before the page even loaded.
        this.lastToastedScanId = latest?.id ?? null;
        this.initialized = true;
      }

      this.previousCurrent = current;
    } catch (e) {
      this.error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      if (this.subscribers > 0) {
        const delay = this.current ? ACTIVE_INTERVAL_MS : IDLE_INTERVAL_MS;
        this.timer = setTimeout(() => void this.tick(), delay);
      }
    }
  }

  /// Compose a result-flavored toast for a scan that just finished.
  /// `failed` and `running` runs don't fire this — only `completed`,
  /// and only when the run actually saw files (a no-op rescan against
  /// an empty needs_review pool is technical noise, not user-relevant
  /// activity).
  private fireScanCompletionToast(run: ScanRun): void {
    if (run.status !== 'completed') return;
    if (run.files_seen === 0 && run.kind !== 'full') return;
    const ownedDelta = run.files_matched;
    toast.success(
      `Scan complete: ${run.files_seen} file${run.files_seen === 1 ? '' : 's'}, ` +
        `${ownedDelta} owned.`
    );
  }
}

export const scanStatus = new ScanStatusStore();
