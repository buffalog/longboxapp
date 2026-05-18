import { listFilters } from '$lib/api/publishers';
import { getSettings } from '$lib/api/settings';

export const load = async () => {
  const [settings, publisherFilters] = await Promise.all([getSettings(), listFilters()]);
  return { settings, publisherFilters };
};
