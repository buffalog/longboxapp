import { getDownloader } from '$lib/api/downloader';
import { listIndexers } from '$lib/api/indexers';
import { listFilters } from '$lib/api/publishers';
import { getSettings } from '$lib/api/settings';
import { listWebhooks } from '$lib/api/webhooks';

export const load = async () => {
  const [settings, publisherFilters, indexers, downloader, webhooks] = await Promise.all([
    getSettings(),
    listFilters(),
    listIndexers(),
    getDownloader(),
    listWebhooks()
  ]);
  return { settings, publisherFilters, indexers, downloader, webhooks };
};
