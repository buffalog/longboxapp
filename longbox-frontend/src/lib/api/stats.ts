import { apiFetch } from './client';
import type { Stats } from '../types';

export function getStats(): Promise<Stats> {
  return apiFetch('/stats');
}
