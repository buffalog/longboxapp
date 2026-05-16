import { listSeries } from '$lib/api/series';

export const load = async () => {
  const series = await listSeries();
  return { series };
};
