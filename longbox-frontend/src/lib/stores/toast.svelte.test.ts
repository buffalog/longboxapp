import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { toast } from './toast.svelte';

// The store is a singleton; each test clears it so state doesn't leak.
function clearAll(): void {
  for (const t of [...toast.toasts]) {
    toast.dismiss(t.id);
  }
  // dismiss() schedules removal; flush the exit timers.
  vi.runAllTimers();
}

describe('toast store', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    clearAll();
  });

  afterEach(() => {
    clearAll();
    vi.useRealTimers();
  });

  it('success() pushes a success toast, newest first', () => {
    toast.success('first');
    toast.success('second');
    expect(toast.toasts.map((t) => t.message)).toEqual(['second', 'first']);
    expect(toast.toasts[0]!.type).toBe('success');
  });

  it('error / info / warning set the right type', () => {
    toast.error('e');
    toast.info('i');
    toast.warning('w');
    const byMsg = Object.fromEntries(toast.toasts.map((t) => [t.message, t.type]));
    expect(byMsg).toEqual({ e: 'error', i: 'info', w: 'warning' });
  });

  it('auto-dismisses success after 3s, error after 5s', () => {
    toast.success('quick');
    toast.error('slow');
    expect(toast.toasts.length).toBe(2);

    // At 3s + exit fade: success gone, error still up.
    vi.advanceTimersByTime(3000 + 150);
    expect(toast.toasts.map((t) => t.message)).toEqual(['slow']);

    // At 5s + exit fade total: error gone too.
    vi.advanceTimersByTime(2000 + 150);
    expect(toast.toasts.length).toBe(0);
  });

  it('caps the stack at 3, evicting the oldest instantly', () => {
    toast.info('1');
    toast.info('2');
    toast.info('3');
    toast.info('4');
    // Newest-first, max 3 → '1' evicted, no exit flag (instant).
    expect(toast.toasts.map((t) => t.message)).toEqual(['4', '3', '2']);
    expect(toast.toasts.length).toBe(3);
  });

  it('dismiss() sets exiting then removes after the fade', () => {
    const id = toast.success('bye');
    toast.dismiss(id);
    // Exiting flag set immediately; still in the array during the fade.
    expect(toast.toasts[0]!.exiting).toBe(true);
    expect(toast.toasts.length).toBe(1);
    // After the 150ms exit window the entry is spliced out.
    vi.advanceTimersByTime(150);
    expect(toast.toasts.length).toBe(0);
  });

  it('dismiss() is idempotent — second call on an exiting toast is a no-op', () => {
    const id = toast.success('x');
    toast.dismiss(id);
    toast.dismiss(id); // must not throw or double-schedule
    vi.advanceTimersByTime(150);
    expect(toast.toasts.length).toBe(0);
  });

  it('show() honors a custom duration override', () => {
    toast.show({ message: 'custom', type: 'success', duration: 10_000 });
    // Default success is 3s — at 4s the custom toast must still be up.
    vi.advanceTimersByTime(4000);
    expect(toast.toasts.length).toBe(1);
    vi.advanceTimersByTime(6000 + 150);
    expect(toast.toasts.length).toBe(0);
  });

  it('carries an action through show()', () => {
    const onClick = vi.fn();
    toast.show({ message: 'undo me', type: 'info', action: { label: 'Undo', onClick } });
    expect(toast.toasts[0]!.action?.label).toBe('Undo');
    toast.toasts[0]!.action?.onClick();
    expect(onClick).toHaveBeenCalledOnce();
  });

  it('assigns unique monotonic ids', () => {
    const a = toast.success('a');
    const b = toast.success('b');
    const c = toast.success('c');
    expect(new Set([a, b, c]).size).toBe(3);
    expect(b).toBeGreaterThan(a);
    expect(c).toBeGreaterThan(b);
  });
});
