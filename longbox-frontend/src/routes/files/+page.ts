import { listFiles } from '$lib/api/files';
import type { FileStatus } from '$lib/types';

const ALLOWED: Array<FileStatus | 'all'> = [
  'needs_review',
  'unmatched',
  'ignored',
  'owned',
  'all'
];

const VIEWS = ['flat', 'folder'] as const;
type View = (typeof VIEWS)[number];

export const load = async ({ url }) => {
  const rawStatus = url.searchParams.get('status') ?? 'needs_review';
  const status = (ALLOWED as string[]).includes(rawStatus)
    ? (rawStatus as FileStatus | 'all')
    : 'needs_review';

  const rawView = url.searchParams.get('view') ?? 'flat';
  const view: View = (VIEWS as readonly string[]).includes(rawView)
    ? (rawView as View)
    : 'flat';

  // Folder filter only applies in folder view; ignored in flat. Empty
  // string is the same as "no filter set" — we don't differentiate.
  const folderFilter = view === 'folder' ? (url.searchParams.get('folder_filter') ?? '') : '';

  const files = await listFiles({ status });
  return { files, status, view, folderFilter };
};
