import { error } from '@sveltejs/kit';
import { ApiError } from '$lib/api/client';
import { getPullEntry } from '$lib/api/pull';
import { getSeries } from '$lib/api/series';

export const load = async ({ params }) => {
  const id = Number(params.id);
  if (!Number.isFinite(id) || id <= 0) {
    error(404, 'invalid series id');
  }
  try {
    // getPullEntry resolves to null for an unsubscribed (or unknown)
    // series, so a 404 here only ever comes from getSeries.
    const [series, pullEntry] = await Promise.all([getSeries(id), getPullEntry(id)]);
    return { series, pullEntry };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      error(404, 'series not found');
    }
    throw e;
  }
};
