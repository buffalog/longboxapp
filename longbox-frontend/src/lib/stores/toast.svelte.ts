// Shared toast store. Singleton, mirrors the class-with-$state pattern
// of scanStatus.svelte.ts. `Toast.svelte` (mounted once in the root
// layout) renders the stack; any module calls `toast.success(...)` etc.
//
// Lifecycle per toast:
//   show()      → unshifted onto the stack (newest on top)
//   +duration   → auto-dismiss fires
//   dismiss(id) → `exiting` flag set, CSS fades over EXIT_MS, then the
//                 entry is spliced out
//   eviction    → when the stack exceeds MAX_VISIBLE, the oldest is
//                 spliced immediately with NO exit animation (the
//                 component declares no Svelte `out:` transition, so a
//                 plain splice vanishes instantly)

export type ToastType = 'success' | 'error' | 'info' | 'warning';

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface Toast {
  id: number;
  message: string;
  type: ToastType;
  action?: ToastAction;
  /** True during the exit fade. Set by dismiss(); the component keys
   *  its fade-out CSS off this. */
  exiting: boolean;
}

export interface ShowOptions {
  message: string;
  type: ToastType;
  /** Override the per-type default auto-dismiss duration (ms). */
  duration?: number;
  action?: ToastAction;
}

/** At most this many toasts visible at once; older ones are evicted. */
const MAX_VISIBLE = 3;

/** Exit-fade duration — must match the CSS transition in Toast.svelte. */
const EXIT_MS = 150;

/** Per-type auto-dismiss defaults: 3 s for low-stakes, 5 s for the
 *  ones the user more likely needs to read. */
const DEFAULT_DURATION: Record<ToastType, number> = {
  success: 3000,
  info: 3000,
  warning: 5000,
  error: 5000,
};

class ToastStore {
  toasts = $state<Toast[]>([]);

  private seq = 0;
  private timers = new Map<number, ReturnType<typeof setTimeout>>();

  /** Low-level entry point. Returns the new toast's id. */
  show(opts: ShowOptions): number {
    const id = ++this.seq;
    const toast: Toast = {
      id,
      message: opts.message,
      type: opts.type,
      action: opts.action,
      exiting: false,
    };
    // Newest on top.
    this.toasts = [toast, ...this.toasts];

    // Evict the oldest beyond the cap — instant, no fade.
    if (this.toasts.length > MAX_VISIBLE) {
      const evicted = this.toasts.slice(MAX_VISIBLE);
      this.toasts = this.toasts.slice(0, MAX_VISIBLE);
      for (const t of evicted) this.clearTimer(t.id);
    }

    const duration = opts.duration ?? DEFAULT_DURATION[opts.type];
    this.timers.set(
      id,
      setTimeout(() => this.dismiss(id), duration)
    );
    return id;
  }

  success(message: string): number {
    return this.show({ message, type: 'success' });
  }

  error(message: string): number {
    return this.show({ message, type: 'error' });
  }

  info(message: string): number {
    return this.show({ message, type: 'info' });
  }

  warning(message: string): number {
    return this.show({ message, type: 'warning' });
  }

  /** Begin the exit fade for a toast, then remove it after EXIT_MS.
   *  No-op if the toast is already exiting or gone. */
  dismiss(id: number): void {
    const toast = this.toasts.find((t) => t.id === id);
    if (!toast || toast.exiting) return;
    toast.exiting = true;
    this.clearTimer(id);
    this.timers.set(
      id,
      setTimeout(() => {
        this.toasts = this.toasts.filter((t) => t.id !== id);
        this.timers.delete(id);
      }, EXIT_MS)
    );
  }

  private clearTimer(id: number): void {
    const t = this.timers.get(id);
    if (t !== undefined) {
      clearTimeout(t);
      this.timers.delete(id);
    }
  }
}

export const toast = new ToastStore();
