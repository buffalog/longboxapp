import { apiFetch } from './client';
import type { LibraryRoot } from '../types';

export function listLibraryRoots(): Promise<LibraryRoot[]> {
  return apiFetch('/library-roots');
}
