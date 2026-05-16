import { error } from '@sveltejs/kit';
import { getSeries } from '$lib/api/series';
import { ApiError } from '$lib/api/client';

export const load = async ({ params }) => {
  const id = Number(params.id);
  if (!Number.isFinite(id) || id <= 0) {
    error(404, 'invalid series id');
  }
  try {
    const series = await getSeries(id);
    return { series };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      error(404, 'series not found');
    }
    throw e;
  }
};
