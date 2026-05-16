import { listFiles } from '$lib/api/files';
import type { FileStatus } from '$lib/types';

const ALLOWED: Array<FileStatus | 'all'> = [
  'needs_review',
  'unmatched',
  'ignored',
  'owned',
  'all'
];

export const load = async ({ url }) => {
  const raw = url.searchParams.get('status') ?? 'needs_review';
  const status = (ALLOWED as string[]).includes(raw) ? (raw as FileStatus | 'all') : 'needs_review';
  const files = await listFiles({ status });
  return { files, status };
};
