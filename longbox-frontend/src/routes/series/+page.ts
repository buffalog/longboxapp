import { listSeries } from '$lib/api/series';

const SORT_VALUES = ['name', 'year', 'added', 'completion'] as const;
export type SeriesSort = (typeof SORT_VALUES)[number];

export const load = async ({ url }) => {
  const raw = url.searchParams.get('sort') ?? 'name';
  const sort: SeriesSort = (SORT_VALUES as readonly string[]).includes(raw)
    ? (raw as SeriesSort)
    : 'name';

  const series = await listSeries();
  return { series, sort };
};
