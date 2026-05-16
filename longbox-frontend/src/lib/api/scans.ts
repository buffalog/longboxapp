import { apiFetch } from './client';
import type { CurrentScan, ScanReport, StartScanResponse } from '../types';

export function getCurrent(): Promise<CurrentScan | null> {
  return apiFetch('/scans/current');
}

export function getRecent(): Promise<ScanReport[]> {
  return apiFetch('/scans/recent');
}

export function triggerFullScan(libraryRootId: number): Promise<StartScanResponse> {
  return apiFetch(`/library-roots/${libraryRootId}/scan`, { method: 'POST' });
}

export function triggerRescanUnmatched(libraryRootId: number): Promise<StartScanResponse> {
  return apiFetch(`/library-roots/${libraryRootId}/rescan-unmatched`, { method: 'POST' });
}
