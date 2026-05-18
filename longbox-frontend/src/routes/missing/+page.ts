import { getMissing, type MissingSort } from '$lib/api/missing';
import { listSeries } from '$lib/api/series';

const SORTS: MissingSort[] = ['series', 'cover_date'];

export const load = async ({ url }) => {
  const rawSort = url.searchParams.get('sort') ?? 'series';
  const sort: MissingSort = (SORTS as string[]).includes(rawSort)
    ? (rawSort as MissingSort)
    : 'series';
  const rawSeriesId = url.searchParams.get('series_id');
  const series_id = rawSeriesId !== null ? Number(rawSeriesId) : undefined;
  const seriesIdParam =
    series_id !== undefined && Number.isFinite(series_id) && series_id > 0
      ? series_id
      : undefined;

  // Fetch both in parallel: missing rows + the series list (for the
  // filter dropdown — small list, server already returns it ordered).
  const [missing, allSeries] = await Promise.all([
    getMissing({ series_id: seriesIdParam, sort }),
    listSeries()
  ]);

  return { missing, allSeries, sort, seriesIdFilter: seriesIdParam ?? null };
};
