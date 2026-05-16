// Cross-route scan-status store. Polls /api/scans/current and
// /api/scans/recent at an adaptive interval (2s while scanning, 30s idle).
// Components subscribe in onMount; the store stops polling when no
// subscribers remain.

import { ApiError } from '../api/client';
import * as scansApi from '../api/scans';
import type { CurrentScan, ScanReport } from '../types';

const ACTIVE_INTERVAL_MS = 2000;
const IDLE_INTERVAL_MS = 30000;

class ScanStatusStore {
  current = $state<CurrentScan | null>(null);
  recent = $state<ScanReport[]>([]);
  error = $state<ApiError | null>(null);

  private timer: ReturnType<typeof setTimeout> | null = null;
  private subscribers = 0;

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
      this.current = current;
      this.recent = recent;
      this.error = null;
    } catch (e) {
      this.error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      if (this.subscribers > 0) {
        const delay = this.current ? ACTIVE_INTERVAL_MS : IDLE_INTERVAL_MS;
        this.timer = setTimeout(() => void this.tick(), delay);
      }
    }
  }
}

export const scanStatus = new ScanStatusStore();
